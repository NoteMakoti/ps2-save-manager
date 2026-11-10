//! `.psu` — EMS Memory Adaptor / uLaunchELF / OPL format.
//!
//! A sequence of 512-byte directory entries: the save directory itself
//! (length = file count + 2), "." and "..", then for each file its entry
//! followed by the data padded to a 1024-byte boundary.

use super::Reader;
use crate::dirent::{DirEntry, Mode, DIRENT_SIZE};
use crate::error::{Error, Result};
use crate::save::{Save, SaveFile};

const CLUSTER: usize = 1024;

pub fn looks_like(data: &[u8]) -> bool {
    if data.len() < DIRENT_SIZE * 3 {
        return false;
    }
    let d = DirEntry::parse(&data[..DIRENT_SIZE]);
    let dot = DirEntry::parse(&data[DIRENT_SIZE..2 * DIRENT_SIZE]);
    let dotdot = DirEntry::parse(&data[2 * DIRENT_SIZE..3 * DIRENT_SIZE]);
    d.mode.is_dir() && dot.mode.is_dir() && dotdot.mode.is_dir() && d.length >= 2 && dot.name == "." && dotdot.name == ".."
}

pub fn load(data: &[u8]) -> Result<Save> {
    if !looks_like(data) {
        return Err(Error::CorruptSave("not a .psu file".into()));
    }
    let mut r = Reader::new(data);
    let dir = DirEntry::parse(r.take(DIRENT_SIZE)?);
    r.take(2 * DIRENT_SIZE)?;
    let count = dir.length as usize - 2;
    let mut files = Vec::with_capacity(count);
    for _ in 0..count {
        let e = DirEntry::parse(r.take(DIRENT_SIZE)?);
        if !e.mode.is_file() {
            return Err(Error::CorruptSave(format!("entry '{}' is not a file", e.name)));
        }
        let len = e.length as usize;
        let body = r.take(len)?.to_vec();
        let pad = len.div_ceil(CLUSTER) * CLUSTER - len;
        // some writers omit the padding after the final file
        if r.rest().len() >= pad {
            r.take(pad)?;
        }
        files.push(SaveFile { name: e.name, mode: e.mode, created: e.created, modified: e.modified, data: body });
    }
    Ok(Save { name: dir.name, mode: dir.mode, created: dir.created, modified: dir.modified, files })
}

pub fn save(s: &Save) -> Vec<u8> {
    let mut out = Vec::with_capacity(DIRENT_SIZE * (3 + s.files.len()) + s.data_size() as usize + s.files.len() * CLUSTER);
    let mode = if s.mode.is_dir() { s.mode } else { Mode::save_dir() };
    let mut d = DirEntry::new(&s.name, mode, s.files.len() as u32 + 2, 0, s.created);
    d.modified = s.modified;
    out.extend_from_slice(&d.to_bytes());
    let mut dot = DirEntry::new(".", Mode::save_dir(), 0, 0, s.created);
    dot.modified = s.modified;
    out.extend_from_slice(&dot.to_bytes());
    let mut dotdot = DirEntry::new("..", Mode::save_dir(), 0, 0, s.created);
    dotdot.modified = s.modified;
    out.extend_from_slice(&dotdot.to_bytes());
    for f in &s.files {
        let mode = if f.mode.is_file() { f.mode } else { Mode::save_file() };
        let mut e = DirEntry::new(&f.name, mode, f.data.len() as u32, 0, f.created);
        e.modified = f.modified;
        out.extend_from_slice(&e.to_bytes());
        out.extend_from_slice(&f.data);
        let pad = f.data.len().div_ceil(CLUSTER) * CLUSTER - f.data.len();
        out.resize(out.len() + pad, 0);
    }
    out
}
