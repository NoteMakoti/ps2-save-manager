//! `icon.sys` — per-save metadata shown by the PS2 browser.
//!
//! 964 bytes: magic "PS2D", title line-break offset at 0x06, background
//! colours and lights, a 68-byte Shift-JIS title at 0xC0 and the names of
//! the normal / copy / delete icon files at 0x104, 0x144, 0x184.

use crate::error::{Error, Result};

pub const ICON_SYS_SIZE: usize = 964;

#[derive(Debug, Clone)]
pub struct IconSys {
    pub line_break: u16,
    pub title_raw: Vec<u8>,
    pub icon_normal: String,
    pub icon_copy: String,
    pub icon_delete: String,
    pub bg_transparency: u32,
}

fn cstr(b: &[u8]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..end]).into_owned()
}

/// Decode Shift-JIS and fold full-width ASCII / ideographic space into
/// plain ASCII so titles look natural on a PC.
pub fn decode_sjis(raw: &[u8]) -> String {
    let end = raw.iter().position(|&c| c == 0).unwrap_or(raw.len());
    let (s, _, _) = encoding_rs::SHIFT_JIS.decode(&raw[..end]);
    s.chars()
        .map(|c| match c as u32 {
            0x3000 => ' ',
            0xFF01..=0xFF5E => char::from_u32(c as u32 - 0xFF01 + 0x21).unwrap_or(c),
            0x2019 | 0x2018 | 0x2032 => '\'',
            0x201C | 0x201D => '"',
            0x2010 | 0x2015 | 0x2212 => '-',
            _ => c,
        })
        .collect()
}

impl IconSys {
    pub fn parse(b: &[u8]) -> Result<IconSys> {
        if b.len() < ICON_SYS_SIZE || &b[..4] != b"PS2D" {
            return Err(Error::CorruptSave("bad icon.sys".into()));
        }
        Ok(IconSys {
            line_break: u16::from_le_bytes([b[6], b[7]]),
            bg_transparency: u32::from_le_bytes([b[0xC], b[0xD], b[0xE], b[0xF]]),
            title_raw: b[0xC0..0x104].to_vec(),
            icon_normal: cstr(&b[0x104..0x144]),
            icon_copy: cstr(&b[0x144..0x184]),
            icon_delete: cstr(&b[0x184..0x1C4]),
        })
    }

    /// The two title lines.
    pub fn title_lines(&self) -> (String, String) {
        let end = self.title_raw.iter().position(|&c| c == 0).unwrap_or(self.title_raw.len());
        let lb = (self.line_break as usize).min(end);
        (decode_sjis(&self.title_raw[..lb]), decode_sjis(&self.title_raw[lb..end]))
    }

    /// Title joined into one line with whitespace normalised.
    pub fn title(&self) -> String {
        let (a, b) = self.title_lines();
        format!("{a} {b}").split_whitespace().collect::<Vec<_>>().join(" ")
    }

    /// ASCII-only title (for formats that store a plain title).
    pub fn title_ascii(&self) -> String {
        self.title().chars().filter(|c| c.is_ascii() && !c.is_ascii_control()).collect()
    }
}
