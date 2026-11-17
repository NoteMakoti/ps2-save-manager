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

/// Connection to PS2SM. If a request fails at the network level (the
/// PS2 stopped answering, the server screen was closed and reopened…) the
/// connection is dropped and the next request reconnects, so a late or
/// partial reply can never be mistaken for the answer to a later request.
pub struct Ps2Client {
    addr: String,
    stream: Option<TcpStream>,
    next_id: u32,
    pub info: ServerInfo,
    /// Replaces the per-command reply timeouts (tests, slow setups).
    timeout_override: Option<Duration>,
}

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// How long to wait for the reply to `cmd`. The PS2 replies only after
/// the memory card work is done; writes run at roughly 100–200 KB/s.
fn reply_timeout(cmd: Cmd, payload_len: usize) -> Duration {
    let secs = match cmd {
        Cmd::Hello | Cmd::CardInfo | Cmd::MmceStatus | Cmd::MmceSet | Cmd::Bye | Cmd::Mkdir | Cmd::Delete => 20,
        Cmd::ListDir | Cmd::ListSaves | Cmd::DeleteSave => 90,
        Cmd::ReadFile | Cmd::GetSave => 120,
        Cmd::WriteFile | Cmd::PutSave => 60 + (payload_len / (32 * 1024)) as u64,
        Cmd::Format => 120,
    };
    Duration::from_secs(secs)
}

/// Requests that only read, so they can safely be sent again on a fresh
/// connection after a connection-level failure.
fn is_read_only(cmd: Cmd) -> bool {
    matches!(cmd, Cmd::Hello | Cmd::CardInfo | Cmd::ListDir | Cmd::ListSaves | Cmd::ReadFile | Cmd::GetSave | Cmd::MmceStatus)
}

fn cmd_name(cmd: Cmd) -> &'static str {
    match cmd {
        Cmd::Hello => "connecting",
        Cmd::CardInfo => "checking the memory card",
        Cmd::ListDir => "listing files",
        Cmd::ReadFile => "reading a file",
        Cmd::WriteFile => "writing a file",
        Cmd::Mkdir => "creating a folder",
        Cmd::Delete => "deleting a file",
        Cmd::GetSave => "reading a save",
        Cmd::PutSave => "writing a save",
        Cmd::DeleteSave => "deleting a save",
        Cmd::Format => "formatting the card",
        Cmd::ListSaves => "listing saves",
        Cmd::MmceStatus | Cmd::MmceSet => "talking to the MMCE device",
        Cmd::Bye => "disconnecting",
    }
}

/// Turn a socket error into something a person can act on.
fn io_error(e: std::io::Error, cmd: Cmd, timeout: Duration) -> NetError {
    use std::io::ErrorKind::*;
    match e.kind() {
        WouldBlock | TimedOut => NetError::Timeout { what: cmd_name(cmd), secs: timeout.as_secs() },
        ConnectionReset | ConnectionAborted | BrokenPipe | UnexpectedEof | NotConnected => NetError::Disconnected(format!(
            "the PS2 closed the connection while {} (is PS2SM still on the Network server screen?)",
            cmd_name(cmd)
        )),
        _ => NetError::Io(e),
    }
}

impl Ps2Client {
    /// Connect to `host` or `host:port` (default port 6524).
    pub fn connect(addr: &str) -> NetResult<Ps2Client> {
        let addr = if addr.contains(':') { addr.to_string() } else { format!("{addr}:{}", proto::DEFAULT_PORT) };
        let mut c = Ps2Client {
            addr,
            stream: None,
            next_id: 1,
            info: ServerInfo { protocol: 0, name: String::new(), version: String::new(), caps: 0 },
            timeout_override: None,
        };
        c.reconnect()?;
        Ok(c)
    }

    /// Use one reply timeout for every command instead of the defaults.
    pub fn set_reply_timeout(&mut self, t: Option<Duration>) {
        self.timeout_override = t;
    }

    /// The address this client talks to.
    pub fn addr(&self) -> &str {
        &self.addr
    }

    /// True while a TCP connection is open (it is re-opened on demand).
    pub fn is_connected(&self) -> bool {
        self.stream.is_some()
    }

    fn reconnect(&mut self) -> NetResult<()> {
        self.stream = None;
        let sa = self
            .addr
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| NetError::Protocol(format!("cannot resolve {}", self.addr)))?;
        let stream = TcpStream::connect_timeout(&sa, CONNECT_TIMEOUT).map_err(|e| match e.kind() {
            std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => NetError::Disconnected(format!(
                "no answer from {} (check the IP, and that PS2SM shows the Network server screen)",
                self.addr
            )),
            std::io::ErrorKind::ConnectionRefused => NetError::Disconnected(format!(
                "{} refused the connection: PS2SM's Network server is not running",
                self.addr
            )),
            _ => NetError::Io(e),
        })?;
        stream.set_nodelay(true).ok();
        stream.set_write_timeout(Some(Duration::from_secs(30)))?;
        self.stream = Some(stream);
        let p = self.exchange(Cmd::Hello, &Out::new().str("ps2sm-desktop").0)?;
        let mut i = In::new(&p);
        self.info = ServerInfo { protocol: i.u16()?, name: i.str()?, version: i.str()?, caps: i.u32()? };
        Ok(())
    }

    /// One request/response on the current connection. Any failure other
    /// than an error reported by the PS2 drops the connection.
    fn exchange(&mut self, cmd: Cmd, payload: &[u8]) -> NetResult<Vec<u8>> {
        let r = self.exchange_inner(cmd, payload);
        if let Err(e) = &r {
            if !matches!(e, NetError::Remote { .. }) {
                self.stream = None;
            }
        }
        r
    }

    fn exchange_inner(&mut self, cmd: Cmd, payload: &[u8]) -> NetResult<Vec<u8>> {
        let timeout = self.timeout_override.unwrap_or_else(|| reply_timeout(cmd, payload.len()));
        let stream = self
            .stream
            .as_mut()
            .ok_or_else(|| NetError::Disconnected("not connected".into()))?;
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        stream.set_read_timeout(Some(timeout))?;
        write_frame(stream, proto::REQ_MAGIC, cmd as u8, 0, id, payload).map_err(|e| match e {
            NetError::Io(e) => io_error(e, cmd, timeout),
            e => e,
        })?;
        let f = read_frame(stream, proto::RESP_MAGIC)
            .map_err(|e| match e {
                NetError::Io(e) => io_error(e, cmd, timeout),
                e => e,
            })?
            .ok_or_else(|| {
                NetError::Disconnected(format!(
                    "the PS2 closed the connection while {} (is PS2SM still on the Network server screen?)",
                    cmd_name(cmd)
                ))
            })?;
        if f.id != id || f.cmd != cmd as u8 {
            return Err(NetError::Protocol(format!("unexpected response (cmd {:#x}, id {})", f.cmd, f.id)));
        }
        if f.status != 0 {
            let message = In::new(&f.payload).str().unwrap_or_default();
            return Err(NetError::Remote { code: f.status, message });
        }
        Ok(f.payload)
    }

    fn call(&mut self, cmd: Cmd, payload: &[u8]) -> NetResult<Vec<u8>> {
        let had_connection = self.stream.is_some();
        if !had_connection {
            self.reconnect()?;
        }
        match self.exchange(cmd, payload) {
            // An old connection may have been closed by the PS2 in the
            // meantime; read-only requests are retried once on a new one.
            Err(NetError::Disconnected(_)) if had_connection && is_read_only(cmd) => {
                self.reconnect()?;
                self.exchange(cmd, payload)
            }
            r => r,
        }
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
        if self.stream.is_some() {
            let _ = self.exchange(Cmd::Bye, &[]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// A fake PS2: answers HELLO; CARD_INFO is answered only on
    /// connections after the first `freeze_conns` ones.
    fn fake_ps2(freeze_conns: usize, close_after_hello: usize) -> (String, Arc<AtomicUsize>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap().to_string();
        let conns = Arc::new(AtomicUsize::new(0));
        let c2 = conns.clone();
        std::thread::spawn(move || {
            for s in l.incoming() {
                let mut s = s.unwrap();
                let n = c2.fetch_add(1, Ordering::SeqCst);
                std::thread::spawn(move || loop {
                    let f = match read_frame(&mut s, proto::REQ_MAGIC) {
                        Ok(Some(f)) => f,
                        _ => return,
                    };
                    let p = match Cmd::from_u8(f.cmd).unwrap() {
                        Cmd::Hello => Out::new().u16(1).str("fake").str("1").u32(1).0,
                        Cmd::CardInfo if n < freeze_conns => continue, // never answer
                        Cmd::CardInfo => Out::new().u8(1).u8(2).u8(1).u8(0).u32(100).u32(8000).0,
                        _ => vec![],
                    };
                    write_frame(&mut s, proto::RESP_MAGIC, f.cmd, 0, f.id, &p).unwrap();
                    if f.cmd == Cmd::Hello as u8 && n < close_after_hello {
                        let _ = s.flush();
                        return; // drop the connection right after HELLO
                    }
                });
            }
        });
        (addr, conns)
    }

    #[test]
    fn timeout_drops_connection_and_next_call_reconnects() {
        let (addr, conns) = fake_ps2(1, 0);
        let mut c = Ps2Client::connect(&addr).unwrap();
        c.set_reply_timeout(Some(Duration::from_millis(300)));
        let t0 = std::time::Instant::now();
        let e = c.card_info(0).unwrap_err();
        assert!(matches!(e, NetError::Timeout { .. }), "{e}");
        assert!(t0.elapsed() < Duration::from_secs(3));
        assert!(!c.is_connected());
        // the next call opens a fresh connection and works
        let i = c.card_info(0).unwrap();
        assert!(i.present);
        assert_eq!(conns.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn read_only_request_survives_server_restart() {
        // first connection is closed by the server right after HELLO
        let (addr, conns) = fake_ps2(0, 1);
        let mut c = Ps2Client::connect(&addr).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        let i = c.card_info(0).unwrap();
        assert!(i.present);
        assert_eq!(conns.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn refused_connection_has_clear_message() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap().to_string();
        drop(l);
        let e = Ps2Client::connect(&addr).err().unwrap();
        assert!(e.to_string().contains("Network server is not running"), "{e}");
    }
}
