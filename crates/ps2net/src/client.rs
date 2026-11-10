//! Client for the PS2SM homebrew server.

use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use ps2mc::formats::psu;
use ps2mc::iconsys::decode_sjis;
use ps2mc::{Mode, Save, Tod};

use crate::proto::{self, read_frame, write_frame, Cmd, In, Out};
use crate::{NetError, NetResult};

#[derive(Debug, Clone)]
pub struct ServerInfo {
    pub protocol: u16,
    pub name: String,
    pub version: String,
    pub caps: u32,
}

#[derive(Debug, Clone, Default)]
pub struct RemoteCardInfo {
    pub present: bool,
    /// 0 none, 1 PS1, 2 PS2, 3 PocketStation
    pub card_type: u8,
    pub formatted: bool,
    pub free_kb: u32,
    pub total_kb: u32,
}

#[derive(Debug, Clone)]
pub struct RemoteEntry {
    pub name: String,
    pub mode: Mode,
    pub size: u32,
    pub created: Tod,
    pub modified: Tod,
}

#[derive(Debug, Clone)]
pub struct RemoteSave {
    pub name: String,
    pub title: String,
    pub size: u32,
    pub modified: Tod,
}

#[derive(Debug, Clone)]
pub struct MmceStatus {
    pub card: i32,
    pub channel: i32,
    pub game_id: String,
}

#[derive(Debug, Clone, Copy)]
pub enum MmceTarget {
    Card,
    Channel,
}

#[derive(Debug, Clone, Copy)]
pub enum MmceMode {
    Set(u16),
    Next,
    Prev,
}

pub struct Ps2Client {
    stream: TcpStream,
    next_id: u32,
    pub info: ServerInfo,
}

impl Ps2Client {
    /// Connect to `host` or `host:port` (default port 6524).
    pub fn connect(addr: &str) -> NetResult<Ps2Client> {
        let addr = if addr.contains(':') { addr.to_string() } else { format!("{addr}:{}", proto::DEFAULT_PORT) };
        let sa = addr
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| NetError::Protocol(format!("cannot resolve {addr}")))?;
        let stream = TcpStream::connect_timeout(&sa, Duration::from_secs(5))?;
        stream.set_nodelay(true).ok();
        // card operations on the PS2 are slow (≈ 100 KB/s writes)
        stream.set_read_timeout(Some(Duration::from_secs(120)))?;
        let mut c = Ps2Client {
            stream,
            next_id: 1,
            info: ServerInfo { protocol: 0, name: String::new(), version: String::new(), caps: 0 },
        };
        let p = c.call(Cmd::Hello, &Out::new().str("ps2sm-desktop").0)?;
        let mut i = In::new(&p);
        c.info = ServerInfo { protocol: i.u16()?, name: i.str()?, version: i.str()?, caps: i.u32()? };
        Ok(c)
    }

    fn call(&mut self, cmd: Cmd, payload: &[u8]) -> NetResult<Vec<u8>> {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        write_frame(&mut self.stream, proto::REQ_MAGIC, cmd as u8, 0, id, payload)?;
        let f = read_frame(&mut self.stream, proto::RESP_MAGIC)?
            .ok_or_else(|| NetError::Protocol("server closed the connection".into()))?;
        if f.id != id || f.cmd != cmd as u8 {
            return Err(NetError::Protocol(format!("unexpected response (cmd {:#x}, id {})", f.cmd, f.id)));
        }
        if f.status != 0 {
            let message = In::new(&f.payload).str().unwrap_or_default();
            return Err(NetError::Remote { code: f.status, message });
        }
        Ok(f.payload)
    }

    pub fn card_info(&mut self, port: u8) -> NetResult<RemoteCardInfo> {
        let p = self.call(Cmd::CardInfo, &[port])?;
        let mut i = In::new(&p);
        let present = i.u8()? != 0;
        let card_type = i.u8()?;
        let formatted = i.u8()? != 0;
        i.u8()?;
        Ok(RemoteCardInfo { present, card_type, formatted, free_kb: i.u32()?, total_kb: i.u32()? })
    }

    pub fn list_dir(&mut self, port: u8, path: &str) -> NetResult<Vec<RemoteEntry>> {
        let p = self.call(Cmd::ListDir, &Out::new().u8(port).str(path).0)?;
        let mut i = In::new(&p);
        let n = i.u32()?;
        let mut v = Vec::with_capacity(n as usize);
        for _ in 0..n {
            v.push(RemoteEntry {
                name: i.str()?,
                mode: Mode(i.u16()?),
                size: i.u32()?,
                created: i.tod()?,
                modified: i.tod()?,
            });
        }
        Ok(v)
    }

    pub fn list_saves(&mut self, port: u8) -> NetResult<Vec<RemoteSave>> {
        let p = self.call(Cmd::ListSaves, &[port])?;
        let mut i = In::new(&p);
        let n = i.u32()?;
        let mut v = Vec::with_capacity(n as usize);
        for _ in 0..n {
            let name = i.str()?;
            let raw_title = i.bytes_str()?;
            let title = decode_sjis(raw_title).split_whitespace().collect::<Vec<_>>().join(" ");
            v.push(RemoteSave { name, title, size: i.u32()?, modified: i.tod()? });
        }
        Ok(v)
    }

    pub fn read_file(&mut self, port: u8, path: &str) -> NetResult<Vec<u8>> {
        self.call(Cmd::ReadFile, &Out::new().u8(port).str(path).0)
    }

    pub fn write_file(&mut self, port: u8, path: &str, data: &[u8]) -> NetResult<()> {
        self.call(Cmd::WriteFile, &Out::new().u8(port).str(path).u32(data.len() as u32).raw(data).0)?;
        Ok(())
    }

    pub fn mkdir(&mut self, port: u8, path: &str) -> NetResult<()> {
        self.call(Cmd::Mkdir, &Out::new().u8(port).str(path).0)?;
        Ok(())
    }

    pub fn delete(&mut self, port: u8, path: &str) -> NetResult<()> {
        self.call(Cmd::Delete, &Out::new().u8(port).str(path).0)?;
        Ok(())
    }

    /// Download a whole save.
    pub fn get_save(&mut self, port: u8, dir: &str) -> NetResult<Save> {
        let p = self.call(Cmd::GetSave, &Out::new().u8(port).str(dir).0)?;
        Ok(psu::load(&p)?)
    }

    /// Upload a whole save.
    pub fn put_save(&mut self, port: u8, save: &Save, overwrite: bool) -> NetResult<()> {
        let body = psu::save(save);
        self.call(Cmd::PutSave, &Out::new().u8(port).u8(overwrite as u8).raw(&body).0)?;
        Ok(())
    }

    pub fn delete_save(&mut self, port: u8, dir: &str) -> NetResult<()> {
        self.call(Cmd::DeleteSave, &Out::new().u8(port).str(dir).0)?;
        Ok(())
    }

    pub fn format(&mut self, port: u8) -> NetResult<()> {
        self.call(Cmd::Format, &[port])?;
        Ok(())
    }

    pub fn mmce_status(&mut self, port: u8) -> NetResult<MmceStatus> {
        let p = self.call(Cmd::MmceStatus, &[port])?;
        let mut i = In::new(&p);
        Ok(MmceStatus { card: i.i32()?, channel: i.i32()?, game_id: i.str()? })
    }

    pub fn mmce_set(&mut self, port: u8, target: MmceTarget, mode: MmceMode) -> NetResult<i32> {
        let (m, n) = match mode {
            MmceMode::Set(n) => (0u8, n),
            MmceMode::Next => (1, 0),
            MmceMode::Prev => (2, 0),
        };
        let what = match target {
            MmceTarget::Card => 0u8,
            MmceTarget::Channel => 1,
        };
        let p = self.call(Cmd::MmceSet, &Out::new().u8(port).u8(what).u8(m).u8(0).u16(n).0)?;
        In::new(&p).i32()
    }

    /// Copy every save on a remote card into a fresh local card image.
    pub fn dump_to_image(&mut self, port: u8, with_ecc: bool, mut progress: impl FnMut(usize, usize, &str)) -> NetResult<ps2mc::MemoryCard> {
        let saves = self.list_saves(port)?;
        let mut card = ps2mc::MemoryCard::format(8, with_ecc)?;
        for (n, s) in saves.iter().enumerate() {
            progress(n, saves.len(), &s.name);
            let save = self.get_save(port, &s.name)?;
            card.import_save(&save, true)?;
        }
        progress(saves.len(), saves.len(), "");
        Ok(card)
    }

    pub fn bye(mut self) {
        let _ = self.call(Cmd::Bye, &[]);
    }
}
