//! Wire format shared by the client and the server. See docs/PROTOCOL.md.

use std::io::{Read, Write};

use ps2mc::Tod;

use crate::{NetError, NetResult};

pub const DEFAULT_PORT: u16 = 6524;
pub const VERSION: u8 = 1;
pub const REQ_MAGIC: &[u8; 4] = b"PS2S";
pub const RESP_MAGIC: &[u8; 4] = b"PS2R";
pub const MAX_PAYLOAD: usize = 16 * 1024 * 1024;

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cmd {
    Hello = 0x01,
    CardInfo = 0x02,
    ListDir = 0x03,
    ReadFile = 0x04,
    WriteFile = 0x05,
    Mkdir = 0x06,
    Delete = 0x07,
    GetSave = 0x08,
    PutSave = 0x09,
    DeleteSave = 0x0A,
    Format = 0x0B,
    ListSaves = 0x0C,
    MmceStatus = 0x0D,
    MmceSet = 0x0E,
    Bye = 0x0F,
}

impl Cmd {
    pub fn from_u8(v: u8) -> Option<Cmd> {
        use Cmd::*;
        Some(match v {
            0x01 => Hello,
            0x02 => CardInfo,
            0x03 => ListDir,
            0x04 => ReadFile,
            0x05 => WriteFile,
            0x06 => Mkdir,
            0x07 => Delete,
            0x08 => GetSave,
            0x09 => PutSave,
            0x0A => DeleteSave,
            0x0B => Format,
            0x0C => ListSaves,
            0x0D => MmceStatus,
            0x0E => MmceSet,
            0x0F => Bye,
            _ => return None,
        })
    }
}

pub mod status {
    pub const OK: i16 = 0;
    pub const FAIL: i16 = -1;
    pub const NOT_FOUND: i16 = -2;
    pub const NO_CARD: i16 = -3;
    pub const NO_SPACE: i16 = -4;
    pub const EXISTS: i16 = -5;
    pub const BAD_REQUEST: i16 = -6;
    pub const UNSUPPORTED: i16 = -7;
    pub const IO: i16 = -8;
}

pub mod caps {
    pub const MEMCARD: u32 = 1 << 0;
    pub const MMCE: u32 = 1 << 1;
    pub const USB: u32 = 1 << 2;
    pub const SMB: u32 = 1 << 3;
}

#[derive(Debug, Clone)]
pub struct Frame {
    pub cmd: u8,
    pub status: i16,
    pub id: u32,
    pub payload: Vec<u8>,
}

pub fn write_frame(w: &mut impl Write, magic: &[u8; 4], cmd: u8, status: i16, id: u32, payload: &[u8]) -> NetResult<()> {
    let mut h = [0u8; 16];
    h[..4].copy_from_slice(magic);
    h[4] = VERSION;
    h[5] = cmd;
    h[6..8].copy_from_slice(&status.to_le_bytes());
    h[8..12].copy_from_slice(&id.to_le_bytes());
    h[12..16].copy_from_slice(&(payload.len() as u32).to_le_bytes());
    w.write_all(&h)?;
    w.write_all(payload)?;
    w.flush()?;
    Ok(())
}

/// Read one frame. Returns `Ok(None)` on a clean EOF before the header.
pub fn read_frame(r: &mut impl Read, magic: &[u8; 4]) -> NetResult<Option<Frame>> {
    let mut h = [0u8; 16];
    let mut got = 0;
    while got < 16 {
        let n = r.read(&mut h[got..])?;
        if n == 0 {
            if got == 0 {
                return Ok(None);
            }
            return Err(NetError::Protocol("connection closed mid-header".into()));
        }
        got += n;
    }
    if &h[..4] != magic {
        return Err(NetError::Protocol(format!("bad magic {:02x?}", &h[..4])));
    }
    if h[4] != VERSION {
        return Err(NetError::Protocol(format!("unsupported protocol version {}", h[4])));
    }
    let len = u32::from_le_bytes([h[12], h[13], h[14], h[15]]) as usize;
    if len > MAX_PAYLOAD {
        return Err(NetError::Protocol(format!("payload too large ({len} bytes)")));
    }
    let mut payload = vec![0u8; len];
    r.read_exact(&mut payload)?;
    Ok(Some(Frame {
        cmd: h[5],
        status: i16::from_le_bytes([h[6], h[7]]),
        id: u32::from_le_bytes([h[8], h[9], h[10], h[11]]),
        payload,
    }))
}

/// Payload builder.
#[derive(Default)]
pub struct Out(pub Vec<u8>);

impl Out {
    pub fn new() -> Out {
        Out(Vec::new())
    }
    pub fn u8(mut self, v: u8) -> Self {
        self.0.push(v);
        self
    }
    pub fn u16(mut self, v: u16) -> Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    pub fn u32(mut self, v: u32) -> Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    pub fn i32(mut self, v: i32) -> Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    pub fn str(self, s: &str) -> Self {
        self.bytes_str(s.as_bytes())
    }
    pub fn bytes_str(mut self, b: &[u8]) -> Self {
        self.0.extend_from_slice(&(b.len() as u16).to_le_bytes());
        self.0.extend_from_slice(b);
        self
    }
    pub fn tod(mut self, t: Tod) -> Self {
        self.0.extend_from_slice(&t.to_bytes());
        self
    }
    pub fn raw(mut self, b: &[u8]) -> Self {
        self.0.extend_from_slice(b);
        self
    }
}

/// Payload parser.
pub struct In<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> In<'a> {
    pub fn new(b: &'a [u8]) -> In<'a> {
        In { b, pos: 0 }
    }
    fn take(&mut self, n: usize) -> NetResult<&'a [u8]> {
        if self.pos + n > self.b.len() {
            return Err(NetError::Protocol("truncated payload".into()));
        }
        let s = &self.b[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }
    pub fn u8(&mut self) -> NetResult<u8> {
        Ok(self.take(1)?[0])
    }
    pub fn u16(&mut self) -> NetResult<u16> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }
    pub fn u32(&mut self) -> NetResult<u32> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    pub fn i32(&mut self) -> NetResult<i32> {
        Ok(self.u32()? as i32)
    }
    pub fn bytes_str(&mut self) -> NetResult<&'a [u8]> {
        let n = self.u16()? as usize;
        self.take(n)
    }
    pub fn str(&mut self) -> NetResult<String> {
        Ok(String::from_utf8_lossy(self.bytes_str()?).into_owned())
    }
    pub fn tod(&mut self) -> NetResult<Tod> {
        Ok(Tod::from_bytes(self.take(8)?))
    }
    pub fn rest(&mut self) -> &'a [u8] {
        let s = &self.b[self.pos..];
        self.pos = self.b.len();
        s
    }
}
