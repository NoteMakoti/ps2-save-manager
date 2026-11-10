//! `.sps` / `.xps` — SharkPort / X-Port (GameShark tools).
//!
//! `u32 13, "SharkPortSave"`, u32 save type, three length-prefixed
//! strings (directory name, date, comment), u32 total length, then a
//! 98+ byte header for the directory followed by one header + data per
//! file, and a trailing 4-byte checksum. Mode fields are byte-swapped.

use super::{cstr, put_fixed, Reader};
use crate::dirent::Mode;
use crate::error::{Error, Result};
use crate::save::{Save, SaveFile};
use crate::tod::Tod;

pub const MAGIC: &[u8] = b"\x0d\0\0\0SharkPortSave";
const ENTRY_HDR: usize = 98;

fn read_long_string(r: &mut Reader) -> Result<String> {
    let n = r.u32()? as usize;
    if n > 4096 {
        return Err(Error::CorruptSave("string too long".into()));
    }
    Ok(cstr(r.take(n)?))
}

struct Hdr {
    name: String,
    len: u32,
    mode: Mode,
    created: Tod,
    modified: Tod,
}

fn read_hdr(r: &mut Reader) -> Result<Hdr> {
    let hlen = r.u16()? as usize;
    if hlen < ENTRY_HDR {
        return Err(Error::CorruptSave("entry header too short".into()));
    }
    let name = cstr(r.take(64)?);
    let len = r.u32()?;
    r.take(8)?;
    let m = r.u16()?;
    r.take(2)?;
    let created = Tod::from_bytes(r.take(8)?);
    let modified = Tod::from_bytes(r.take(8)?);
    r.take(hlen - ENTRY_HDR)?;
    Ok(Hdr { name, len, mode: Mode(m.swap_bytes()), created, modified })
}

pub fn load(data: &[u8]) -> Result<Save> {
    let mut r = Reader::new(data);
    if r.take(MAGIC.len())? != MAGIC {
        return Err(Error::CorruptSave("not a SharkPort save".into()));
    }
    let _save_type = r.u32()?;
    let _dirname = read_long_string(&mut r)?;
    let _date = read_long_string(&mut r)?;
    let _comment = read_long_string(&mut r)?;
    let _flen = r.u32()?;
    let dir = read_hdr(&mut r)?;
    if !dir.mode.is_dir() || dir.len < 2 {
        return Err(Error::CorruptSave("bad directory header".into()));
    }
    let now = Tod::now();
    let fix = |t: Tod| if t.is_zero() { now } else { t };
    let mut files = Vec::new();
    for _ in 0..dir.len - 2 {
        let h = read_hdr(&mut r)?;
        if !h.mode.is_file() {
            return Err(Error::CorruptSave(format!("'{}' is not a file", h.name)));
        }
        let body = r.take(h.len as usize)?.to_vec();
        files.push(SaveFile { name: h.name, mode: h.mode, created: fix(h.created), modified: fix(h.modified), data: body });
    }
    Ok(Save { name: dir.name, mode: dir.mode, created: fix(dir.created), modified: fix(dir.modified), files })
}

fn put_long_string(out: &mut Vec<u8>, s: &str) {
    let b: Vec<u8> = s.chars().filter(|c| c.is_ascii()).map(|c| c as u8).collect();
    out.extend_from_slice(&(b.len() as u32).to_le_bytes());
    out.extend_from_slice(&b);
}

fn put_hdr(out: &mut Vec<u8>, name: &str, len: u32, mode: Mode, created: Tod, modified: Tod) {
    out.extend_from_slice(&(ENTRY_HDR as u16).to_le_bytes());
    put_fixed(out, name, 64);
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(&[0u8; 8]);
    out.extend_from_slice(&mode.0.swap_bytes().to_le_bytes());
    out.extend_from_slice(&[0u8; 2]);
    out.extend_from_slice(&created.to_bytes());
    out.extend_from_slice(&modified.to_bytes());
}

/// Write a SharkPort save. The trailing checksum is not documented
/// publicly and is written as zero; readers we know of ignore it.
pub fn save(s: &Save) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&2u32.to_le_bytes());
    put_long_string(&mut out, &s.name);
    let m = s.modified;
    put_long_string(&mut out, &format!("{:02}/{:02}/{:04} {:02}:{:02}", m.day, m.month, m.year, m.hour, m.min));
    put_long_string(&mut out, &s.icon_sys().map(|i| i.title_ascii()).unwrap_or_default());
    let mut body = Vec::new();
    let mode = if s.mode.is_dir() { s.mode } else { Mode::save_dir() };
    put_hdr(&mut body, &s.name, s.files.len() as u32 + 2, mode, s.created, s.modified);
    for f in &s.files {
        let mode = if f.mode.is_file() { f.mode } else { Mode::save_file() };
        put_hdr(&mut body, &f.name, f.data.len() as u32, mode, f.created, f.modified);
        body.extend_from_slice(&f.data);
    }
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    out.extend_from_slice(&0u32.to_le_bytes());
    out
}
