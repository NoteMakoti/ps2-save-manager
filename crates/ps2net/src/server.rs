//! A PS2SM protocol server backed by card image files.
//!
//! It behaves like the PS2 homebrew server, which makes it useful to test
//! the desktop app without a console, and to share a card image on the
//! LAN. Changes are written back to the image file after each mutation.

use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use ps2mc::formats::psu;
use ps2mc::{Error as CardError, MemoryCard};

use crate::proto::{self, caps, read_frame, status, write_frame, Cmd, In, Out};
use crate::{NetError, NetResult};

pub struct Slot {
    pub card: MemoryCard,
    pub path: Option<PathBuf>,
}

pub type Slots = Arc<Mutex<[Option<Slot>; 2]>>;

pub fn slots(a: Option<Slot>, b: Option<Slot>) -> Slots {
    Arc::new(Mutex::new([a, b]))
}

struct Reply {
    status: i16,
    payload: Vec<u8>,
}

fn ok(p: Vec<u8>) -> Reply {
    Reply { status: status::OK, payload: p }
}

fn err(code: i16, msg: impl AsRef<str>) -> Reply {
    Reply { status: code, payload: Out::new().str(msg.as_ref()).0 }
}

fn card_err(e: CardError) -> Reply {
    let code = match &e {
        CardError::NotFound(_) => status::NOT_FOUND,
        CardError::NoSpace { .. } => status::NO_SPACE,
        CardError::Exists(_) => status::EXISTS,
        CardError::InvalidName(_) | CardError::Invalid(_) => status::BAD_REQUEST,
        _ => status::IO,
    };
    err(code, e.to_string())
}

/// Serve forever on `bind` (e.g. "0.0.0.0:6524").
pub fn serve(bind: &str, slots: Slots, mut log: impl FnMut(String) + Send + 'static) -> NetResult<()> {
    let listener = TcpListener::bind(bind)?;
    log(format!("listening on {}", listener.local_addr()?));
    for conn in listener.incoming() {
        let conn = match conn {
            Ok(c) => c,
            Err(e) => {
                log(format!("accept failed: {e}"));
                continue;
            }
        };
        let peer = conn.peer_addr().map(|a| a.to_string()).unwrap_or_default();
        log(format!("client {peer} connected"));
        let res = handle_client(conn, &slots);
        log(format!("client {peer} disconnected ({})", res.err().map(|e| e.to_string()).unwrap_or_else(|| "ok".into())));
    }
    Ok(())
}

/// Bind on an ephemeral local port and serve in a background thread.
/// Returns the address. Used by tests.
pub fn spawn_local(slots: Slots) -> NetResult<String> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let addr = listener.local_addr()?.to_string();
    std::thread::spawn(move || {
        for conn in listener.incoming().flatten() {
            let _ = handle_client(conn, &slots);
        }
    });
    Ok(addr)
}

fn handle_client(mut s: TcpStream, slots: &Slots) -> NetResult<()> {
    s.set_nodelay(true).ok();
    while let Some(f) = read_frame(&mut s, proto::REQ_MAGIC)? {
        let cmd = Cmd::from_u8(f.cmd);
        let reply = match cmd {
            None => err(status::UNSUPPORTED, format!("unknown command {:#x}", f.cmd)),
            Some(Cmd::Bye) => {
                write_frame(&mut s, proto::RESP_MAGIC, f.cmd, 0, f.id, &[])?;
                return Ok(());
            }
            Some(c) => {
                let mut guard = slots.lock().unwrap();
                dispatch(c, &f.payload, &mut guard).unwrap_or_else(|e| err(status::BAD_REQUEST, e.to_string()))
            }
        };
        write_frame(&mut s, proto::RESP_MAGIC, f.cmd, reply.status, f.id, &reply.payload)?;
    }
    Ok(())
}

fn persist(slot: &mut Slot) -> Result<(), CardError> {
    if let Some(p) = &slot.path {
        let p = p.clone();
        slot.card.save_to(p)?;
    }
    Ok(())
}

fn dispatch(cmd: Cmd, payload: &[u8], slots: &mut [Option<Slot>; 2]) -> NetResult<Reply> {
    let mut i = In::new(payload);
    if cmd == Cmd::Hello {
        let present = slots.iter().any(|s| s.is_some());
        let _ = present;
        return Ok(ok(Out::new()
            .u16(proto::VERSION as u16)
            .str("ps2sm image server")
            .str(env!("CARGO_PKG_VERSION"))
            .u32(caps::MEMCARD)
            .0));
    }
    let port = i.u8()? as usize;
    if port > 1 {
        return Ok(err(status::BAD_REQUEST, "port must be 0 or 1"));
    }
    if cmd == Cmd::CardInfo {
        return Ok(match &slots[port] {
            None => ok(Out::new().u8(0).u8(0).u8(0).u8(0).u32(0).u32(0).0),
            Some(s) => {
                let info = s.card.info().map_err(NetError::Card)?;
                ok(Out::new()
                    .u8(1)
                    .u8(2)
                    .u8(1)
                    .u8(0)
                    .u32((info.free_bytes / 1024) as u32)
                    .u32((info.total_bytes / 1024) as u32)
                    .0)
            }
        });
    }
    if matches!(cmd, Cmd::MmceStatus | Cmd::MmceSet) {
        return Ok(err(status::UNSUPPORTED, "no MMCE device in the image server"));
    }
    let slot = match &mut slots[port] {
        Some(s) => s,
        None => return Ok(err(status::NO_CARD, "no card in this slot")),
    };
    let r = match cmd {
        Cmd::ListDir => {
            let path = i.str()?;
            match slot.card.list(&path) {
                Ok(ents) => {
                    let mut o = Out::new().u32(ents.len() as u32);
                    for e in ents {
                        o = o.str(&e.name).u16(e.mode.0).u32(e.length).tod(e.created).tod(e.modified);
                    }
                    ok(o.0)
                }
                Err(e) => card_err(e),
            }
        }
        Cmd::ListSaves => match slot.card.saves() {
            Ok(saves) => {
                let mut o = Out::new().u32(saves.len() as u32);
                for s in saves {
                    let raw_title = slot
                        .card
                        .read_file(&format!("{}/icon.sys", s.name))
                        .ok()
                        .and_then(|b| ps2mc::iconsys::IconSys::parse(&b).ok())
                        .map(|i| {
                            // send the two lines as raw SJIS joined by a space
                            let end = i.title_raw.iter().position(|&c| c == 0).unwrap_or(i.title_raw.len());
                            let lb = (i.line_break as usize).min(end);
                            let mut v = i.title_raw[..lb].to_vec();
                            v.push(b' ');
                            v.extend_from_slice(&i.title_raw[lb..end]);
                            v
                        })
                        .unwrap_or_default();
                    o = o.str(&s.name).bytes_str(&raw_title).u32(s.size as u32).tod(s.modified);
                }
                ok(o.0)
            }
            Err(e) => card_err(e),
        },
        Cmd::ReadFile => match slot.card.read_file(&i.str()?) {
            Ok(d) => ok(d),
            Err(e) => card_err(e),
        },
        Cmd::WriteFile => {
            let path = i.str()?;
            let len = i.u32()? as usize;
            let data = i.rest();
            if data.len() != len {
                return Ok(err(status::BAD_REQUEST, "length mismatch"));
            }
            mutate(slot, |c| c.write_file(&path, data))
        }
        Cmd::Mkdir => {
            let path = i.str()?;
            mutate(slot, |c| c.mkdir(&path))
        }
        Cmd::Delete => {
            let path = i.str()?;
            mutate(slot, |c| c.remove(&path))
        }
        Cmd::GetSave => match slot.card.export_save(&i.str()?) {
            Ok(s) => ok(psu::save(&s)),
            Err(e) => card_err(e),
        },
        Cmd::PutSave => {
            let overwrite = i.u8()? != 0;
            match psu::load(i.rest()) {
                Ok(save) => mutate(slot, |c| c.import_save(&save, overwrite)),
                Err(e) => card_err(e),
            }
        }
        Cmd::DeleteSave => {
            let dir = i.str()?;
            mutate(slot, |c| c.remove_all(&dir))
        }
        Cmd::Format => {
            let ecc = slot.card.has_ecc();
            match MemoryCard::format(8, ecc) {
                Ok(c) => {
                    slot.card = c;
                    match persist(slot) {
                        Ok(()) => ok(vec![]),
                        Err(e) => card_err(e),
                    }
                }
                Err(e) => card_err(e),
            }
        }
        _ => err(status::UNSUPPORTED, "unsupported"),
    };
    Ok(r)
}

fn mutate(slot: &mut Slot, f: impl FnOnce(&mut MemoryCard) -> Result<(), CardError>) -> Reply {
    match f(&mut slot.card).and_then(|_| persist(slot)) {
        Ok(()) => ok(vec![]),
        Err(e) => card_err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::Ps2Client;
    use ps2mc::{Save, SaveFile};

    #[test]
    fn loopback() {
        let mut card = MemoryCard::format(8, true).unwrap();
        let mut s = Save::new("BASLUS-11111");
        s.files.push(SaveFile::new("data", vec![7; 3000]));
        card.import_save(&s, false).unwrap();
        let addr = spawn_local(slots(Some(Slot { card, path: None }), None)).unwrap();

        let mut c = Ps2Client::connect(&addr).unwrap();
        assert_eq!(c.info.protocol, 1);
        assert!(c.card_info(0).unwrap().present);
        assert!(!c.card_info(1).unwrap().present);
        let saves = c.list_saves(0).unwrap();
        assert_eq!(saves.len(), 1);
        let got = c.get_save(0, "BASLUS-11111").unwrap();
        assert_eq!(got.files[0].data, vec![7; 3000]);

        let mut s2 = got.clone();
        s2.name = "BASLUS-22222".into();
        c.put_save(0, &s2, false).unwrap();
        assert!(matches!(c.put_save(0, &s2, false), Err(NetError::Remote { code: -5, .. })));
        assert_eq!(c.list_saves(0).unwrap().len(), 2);
        assert_eq!(c.list_dir(0, "BASLUS-22222").unwrap().len(), 1);
        c.write_file(0, "BASLUS-22222/extra", b"hi").unwrap();
        assert_eq!(c.read_file(0, "BASLUS-22222/extra").unwrap(), b"hi");
        c.delete_save(0, "BASLUS-11111").unwrap();
        assert!(matches!(c.get_save(0, "BASLUS-11111"), Err(NetError::Remote { code: -2, .. })));
        let img = c.dump_to_image(0, false, |_, _, _| {}).unwrap();
        assert_eq!(img.saves().unwrap().len(), 1);
        c.bye();
    }
}
