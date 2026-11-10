//! `.max` — Action Replay MAX / MAX Drive.
//!
//! Header (0x5C bytes): magic "Ps2PowerSave", CRC32 (computed with this
//! field zeroed), directory name[32], title[32], compressed size + 4,
//! file count, uncompressed size. Then LZARI data which expands to, for
//! each file: `u32 length, name[32], data`, padded so the next record
//! starts at an offset ≡ 8 (mod 16).

use super::{cstr, lzari, put_fixed, Reader};
use crate::error::{Error, Result};
use crate::save::{Save, SaveFile};
use crate::tod::Tod;

pub const MAGIC: &[u8] = b"Ps2PowerSave";
const HEADER: usize = 0x5C;

pub fn load(data: &[u8]) -> Result<Save> {
    let mut r = Reader::new(data);
    if r.take(12)? != MAGIC {
        return Err(Error::CorruptSave("not a MAX save".into()));
    }
    let _crc = r.u32()?;
    let dirname = cstr(r.take(32)?);
    let _title = r.take(32)?;
    let clen = r.u32()? as usize;
    let count = r.u32()? as usize;
    let length = r.u32()? as usize;
    let body = if clen == length || clen < 4 {
        // some writers store the uncompressed size in both fields
        r.rest()
    } else {
        let avail = r.rest().len();
        r.take((clen - 4).min(avail))?
    };
    if length > 256 * 1024 * 1024 {
        return Err(Error::CorruptSave("implausible uncompressed size".into()));
    }
    let raw = lzari::decode(body, length);
    let now = Tod::now();
    let mut files = Vec::with_capacity(count);
    let mut off = 0usize;
    for _ in 0..count {
        if raw.len() < off + 36 {
            return Err(Error::CorruptSave("truncated MAX data".into()));
        }
        let l = u32::from_le_bytes([raw[off], raw[off + 1], raw[off + 2], raw[off + 3]]) as usize;
        let name = cstr(&raw[off + 4..off + 36]);
        off += 36;
        if raw.len() < off + l {
            return Err(Error::CorruptSave(format!("truncated file '{name}'")));
        }
        let mut f = SaveFile::new(&name, raw[off..off + l].to_vec());
        f.created = now;
        f.modified = now;
        files.push(f);
        off += l;
        off = (off + 8).div_ceil(16) * 16 - 8;
    }
    let mut s = Save::new(&dirname);
    s.created = now;
    s.modified = now;
    s.files = files;
    Ok(s)
}

pub fn save(s: &Save) -> Vec<u8> {
    let title = s
        .icon_sys()
        .map(|i| {
            let (a, b) = i.title_lines();
            let a: String = a.chars().filter(|c| c.is_ascii()).collect();
            let b: String = b.chars().filter(|c| c.is_ascii()).collect();
            if !a.is_empty() && !a.ends_with(' ') {
                format!("{} {}", a, b.trim())
            } else {
                format!("{}{}", a, b.trim_end())
            }
        })
        .unwrap_or_default();

    let mut raw = Vec::new();
    for f in &s.files {
        raw.extend_from_slice(&(f.data.len() as u32).to_le_bytes());
        put_fixed(&mut raw, &f.name, 32);
        raw.extend_from_slice(&f.data);
        let padded = (raw.len() + 8).div_ceil(16) * 16 - 8;
        raw.resize(padded, 0);
    }
    let compressed = lzari::encode(&raw);

    let mut hdr = Vec::with_capacity(HEADER);
    hdr.extend_from_slice(MAGIC);
    hdr.extend_from_slice(&0u32.to_le_bytes());
    put_fixed(&mut hdr, &s.name, 32);
    put_fixed(&mut hdr, &title, 32);
    hdr.extend_from_slice(&(compressed.len() as u32 + 4).to_le_bytes());
    hdr.extend_from_slice(&(s.files.len() as u32).to_le_bytes());
    hdr.extend_from_slice(&(raw.len() as u32).to_le_bytes());

    let mut h = crc32fast::Hasher::new();
    h.update(&hdr);
    h.update(&compressed);
    let crc = h.finalize();
    hdr[12..16].copy_from_slice(&crc.to_le_bytes());
    hdr.extend_from_slice(&compressed);
    hdr
}
