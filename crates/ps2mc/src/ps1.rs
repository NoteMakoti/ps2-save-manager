//! PlayStation 1 memory cards (`.mcd`, `.mcr`, `.mc`, `.gme`, `.vgs`).
//!
//! 128 KiB, 16 blocks of 8 KiB, each block 64 frames of 128 bytes. Block
//! 0 is the directory: frame 0 holds "MC", frames 1–15 describe blocks
//! 1–15. A save spans one or more blocks linked through the directory.
//!
//! Directory frame: `u32 state` (0x51 first block, 0x52 middle, 0x53
//! last, 0xA0 free, 0xA1–0xA3 deleted), `u32 size` (first block only),
//! `u16 next` (block index − 1, 0xFFFF = none), `name[21]`, checksum at
//! 0x7F (XOR of bytes 0–0x7E).

use crate::error::{Error, Result};
use crate::iconsys::decode_sjis;

pub const CARD_SIZE: usize = 128 * 1024;
pub const BLOCK_SIZE: usize = 8192;
const FRAME: usize = 128;
const BLOCKS: usize = 15;

const ST_FIRST: u32 = 0x51;
const ST_MIDDLE: u32 = 0x52;
const ST_LAST: u32 = 0x53;
const ST_FREE: u32 = 0xA0;

/// Headers of PC/accessory formats that wrap a raw 128 KiB image.
fn raw_offset(data: &[u8]) -> Option<usize> {
    match data.len() {
        CARD_SIZE if &data[..2] == b"MC" => Some(0),
        n if n == CARD_SIZE + 3904 && data.starts_with(b"123-456-STD") => Some(3904), // .gme (DexDrive)
        n if n == CARD_SIZE + 64 && data.starts_with(b"VgsM") => Some(64),            // .vgs/.mem
        n if n == CARD_SIZE + 3904 => Some(3904),
        _ if data.len() >= CARD_SIZE && &data[..2] == b"MC" => Some(0),
        _ => None,
    }
}

/// True if `data` looks like a PS1 card image.
pub fn looks_like(data: &[u8]) -> bool {
    raw_offset(data).is_some()
}

fn checksum(frame: &[u8]) -> u8 {
    frame[..127].iter().fold(0, |a, b| a ^ b)
}

/// One PS1 save: its directory frame (from the first block) plus data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ps1Save {
    /// File name, e.g. `BASLUS-00067DRAX00`.
    pub name: String,
    /// Whole blocks of data (`blocks * 8192` bytes).
    pub data: Vec<u8>,
}

impl Ps1Save {
    pub fn blocks(&self) -> usize {
        self.data.len().div_ceil(BLOCK_SIZE).max(1)
    }

    /// Title from the save header ("SC" + Shift-JIS title).
    pub fn title(&self) -> String {
        if self.data.len() >= 68 && &self.data[..2] == b"SC" {
            decode_sjis(&self.data[4..68]).split_whitespace().collect::<Vec<_>>().join(" ")
        } else {
            String::new()
        }
    }

    pub fn game_id(&self) -> Option<String> {
        crate::save::game_id_from_dir(&self.name)
    }

    /// `.mcs` single-save file: 128-byte directory frame + data.
    pub fn to_mcs(&self) -> Vec<u8> {
        let mut f = dir_frame(ST_FIRST, self.data.len() as u32, 0xFFFF, &self.name);
        f.extend_from_slice(&self.data);
        f
    }

    pub fn from_mcs(b: &[u8]) -> Result<Ps1Save> {
        if b.len() < FRAME + BLOCK_SIZE || b[0] != ST_FIRST as u8 {
            return Err(Error::CorruptSave("not a .mcs file".into()));
        }
        let size = u32::from_le_bytes([b[4], b[5], b[6], b[7]]) as usize;
        let name = crate::formats::cstr(&b[0x0A..0x1F]);
        let mut data = b[FRAME..].to_vec();
        let want = if size >= BLOCK_SIZE { size } else { data.len() };
        data.resize(want.div_ceil(BLOCK_SIZE) * BLOCK_SIZE, 0);
        Ok(Ps1Save { name, data })
    }

    pub fn suggested_filename(&self, ext: &str) -> String {
        let base: String = self
            .name
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || "-_.".contains(c) { c } else { '_' })
            .collect();
        format!("{base}.{ext}")
    }
}

/// Is this a `.mcs` file?
pub fn is_mcs(b: &[u8]) -> bool {
    b.len() >= FRAME + BLOCK_SIZE && b[0] == ST_FIRST as u8 && (b.len() - FRAME) % BLOCK_SIZE == 0 && checksum(&b[..FRAME]) == b[127]
}

fn dir_frame(state: u32, size: u32, next: u16, name: &str) -> Vec<u8> {
    let mut f = vec![0u8; FRAME];
    f[0..4].copy_from_slice(&state.to_le_bytes());
    f[4..8].copy_from_slice(&size.to_le_bytes());
    f[8..10].copy_from_slice(&next.to_le_bytes());
    for (i, c) in name.bytes().take(20).enumerate() {
        f[0x0A + i] = c;
    }
    f[127] = checksum(&f);
    f
}

#[derive(Debug, Clone)]
pub struct Ps1SaveSummary {
    pub name: String,
    pub title: String,
    pub blocks: usize,
}

pub struct Ps1Card {
    data: Vec<u8>,
    dirty: bool,
}

impl std::fmt::Debug for Ps1Card {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Ps1Card")
    }
}

impl Ps1Card {
    pub fn open(data: Vec<u8>) -> Result<Ps1Card> {
        let off = raw_offset(&data).ok_or_else(|| Error::NotACard("not a PS1 memory card image".into()))?;
        Ok(Ps1Card { data: data[off..off + CARD_SIZE].to_vec(), dirty: false })
    }

    pub fn format() -> Ps1Card {
        let mut d = vec![0u8; CARD_SIZE];
        let mut hdr = vec![0u8; FRAME];
        hdr[0] = b'M';
        hdr[1] = b'C';
        hdr[127] = checksum(&hdr);
        d[..FRAME].copy_from_slice(&hdr);
        for i in 1..=BLOCKS {
            d[i * FRAME..(i + 1) * FRAME].copy_from_slice(&dir_frame(ST_FREE, 0, 0xFFFF, ""));
        }
        // broken sector list: none
        for i in 16..36 {
            let mut f = vec![0u8; FRAME];
            f[0..4].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
            f[8..10].copy_from_slice(&0xFFFFu16.to_le_bytes());
            f[127] = checksum(&f);
            d[i * FRAME..(i + 1) * FRAME].copy_from_slice(&f);
        }
        d[63 * FRAME..64 * FRAME].copy_from_slice(&hdr);
        Ps1Card { data: d, dirty: true }
    }

    /// Raw 128 KiB image (the `.mcd` file).
    pub fn as_bytes(&self) -> &[u8] {
        &self.data
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn mark_clean(&mut self) {
        self.dirty = false;
    }

    pub fn save_to(&mut self, path: impl AsRef<std::path::Path>) -> Result<()> {
        std::fs::write(path, &self.data)?;
        self.dirty = false;
        Ok(())
    }

    fn frame(&self, i: usize) -> &[u8] {
        &self.data[i * FRAME..(i + 1) * FRAME]
    }

    fn state(&self, block: usize) -> u32 {
        let f = self.frame(block);
        u32::from_le_bytes([f[0], f[1], f[2], f[3]])
    }

    fn next(&self, block: usize) -> u16 {
        let f = self.frame(block);
        u16::from_le_bytes([f[8], f[9]])
    }

    fn set_frame(&mut self, block: usize, f: &[u8]) {
        self.data[block * FRAME..(block + 1) * FRAME].copy_from_slice(f);
        self.dirty = true;
    }

    fn block_data(&self, block: usize) -> &[u8] {
        &self.data[block * BLOCK_SIZE..(block + 1) * BLOCK_SIZE]
    }

    /// Blocks (1-based) of the save starting at `first`.
    fn chain(&self, first: usize) -> Result<Vec<usize>> {
        let mut v = vec![first];
        let mut b = first;
        loop {
            let n = self.next(b);
            if n == 0xFFFF {
                break;
            }
            let nb = n as usize + 1;
            if nb > BLOCKS || v.contains(&nb) {
                return Err(Error::Corrupt(format!("bad block link in save at block {first}")));
            }
            v.push(nb);
            b = nb;
        }
        Ok(v)
    }

    fn first_blocks(&self) -> Vec<usize> {
        (1..=BLOCKS).filter(|&b| self.state(b) == ST_FIRST).collect()
    }

    fn find(&self, name: &str) -> Option<usize> {
        self.first_blocks().into_iter().find(|&b| crate::formats::cstr(&self.frame(b)[0x0A..0x1F]) == name)
    }

    pub fn free_blocks(&self) -> usize {
        (1..=BLOCKS).filter(|&b| self.state(b) & 0xF0 == 0xA0).count()
    }

    pub fn saves(&self) -> Vec<Ps1SaveSummary> {
        let mut v = Vec::new();
        for b in self.first_blocks() {
            let name = crate::formats::cstr(&self.frame(b)[0x0A..0x1F]);
            let blocks = self.chain(b).map(|c| c.len()).unwrap_or(1);
            let d = self.block_data(b);
            let title = if &d[..2] == b"SC" {
                decode_sjis(&d[4..68]).split_whitespace().collect::<Vec<_>>().join(" ")
            } else {
                String::new()
            };
            v.push(Ps1SaveSummary { name, title, blocks });
        }
        v
    }

    pub fn export_save(&self, name: &str) -> Result<Ps1Save> {
        let first = self.find(name).ok_or_else(|| Error::NotFound(name.into()))?;
        let mut data = Vec::new();
        for b in self.chain(first)? {
            data.extend_from_slice(self.block_data(b));
        }
        Ok(Ps1Save { name: name.to_string(), data })
    }

    pub fn remove(&mut self, name: &str) -> Result<()> {
        let first = self.find(name).ok_or_else(|| Error::NotFound(name.into()))?;
        for b in self.chain(first)? {
            self.set_frame(b, &dir_frame(ST_FREE, 0, 0xFFFF, ""));
        }
        Ok(())
    }

    pub fn import_save(&mut self, save: &Ps1Save, overwrite: bool) -> Result<()> {
        if save.name.is_empty() || save.name.len() > 20 {
            return Err(Error::InvalidName(save.name.clone()));
        }
        let need = save.blocks();
        let existing = self.find(&save.name);
        let reclaim = match existing {
            Some(_) if !overwrite => return Err(Error::Exists(save.name.clone())),
            Some(f) => self.chain(f)?.len(),
            None => 0,
        };
        let free = self.free_blocks();
        if free + reclaim < need {
            return Err(Error::NoSpace { needed: need as u64 * 8, free: (free + reclaim) as u64 * 8 });
        }
        if existing.is_some() {
            self.remove(&save.name)?;
        }
        let blocks: Vec<usize> = (1..=BLOCKS).filter(|&b| self.state(b) & 0xF0 == 0xA0).take(need).collect();
        for (i, &b) in blocks.iter().enumerate() {
            let next = blocks.get(i + 1).map(|&n| (n - 1) as u16).unwrap_or(0xFFFF);
            let (state, size, name) = if i == 0 {
                (ST_FIRST, (need * BLOCK_SIZE) as u32, save.name.as_str())
            } else if i + 1 == need {
                (ST_LAST, 0, "")
            } else {
                (ST_MIDDLE, 0, "")
            };
            self.set_frame(b, &dir_frame(state, size, next, name));
            let mut chunk = vec![0u8; BLOCK_SIZE];
            let s = i * BLOCK_SIZE;
            let e = (s + BLOCK_SIZE).min(save.data.len());
            if s < e {
                chunk[..e - s].copy_from_slice(&save.data[s..e]);
            }
            self.data[b * BLOCK_SIZE..(b + 1) * BLOCK_SIZE].copy_from_slice(&chunk);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn save(name: &str, blocks: usize, fill: u8) -> Ps1Save {
        let mut data = vec![fill; blocks * BLOCK_SIZE];
        data[..2].copy_from_slice(b"SC");
        let (t, _, _) = encoding_rs::SHIFT_JIS.encode("ＦＦ７　Ｓａｖｅ");
        data[4..4 + t.len()].copy_from_slice(&t);
        data[4 + t.len()..68].fill(0);
        Ps1Save { name: name.into(), data }
    }

    #[test]
    fn card_roundtrip() {
        let mut c = Ps1Card::format();
        assert_eq!(c.free_blocks(), 15);
        c.import_save(&save("BASCUS-94163FF7-S01", 1, 1), false).unwrap();
        c.import_save(&save("BASLUS-00067DRAX00", 3, 2), false).unwrap();
        assert_eq!(c.free_blocks(), 11);
        let c2 = Ps1Card::open(c.as_bytes().to_vec()).unwrap();
        let s = c2.saves();
        assert_eq!(s.len(), 2);
        assert_eq!(s[1].blocks, 3);
        assert_eq!(s[0].title, "FF7 Save");
        let e = c2.export_save("BASLUS-00067DRAX00").unwrap();
        assert_eq!(e.data.len(), 3 * BLOCK_SIZE);
        assert_eq!(e.game_id().as_deref(), Some("SLUS-00067"));
        let mcs = e.to_mcs();
        assert!(is_mcs(&mcs));
        assert_eq!(Ps1Save::from_mcs(&mcs).unwrap(), e);
        c.remove("BASCUS-94163FF7-S01").unwrap();
        assert_eq!(c.free_blocks(), 12);
        assert!(c.import_save(&save("BASLUS-00067DRAX00", 1, 3), false).is_err());
        c.import_save(&save("BIG", 13, 4), false).unwrap_err();
        c.import_save(&save("BIG", 12, 4), false).unwrap();
        assert_eq!(c.free_blocks(), 0);
    }
}
