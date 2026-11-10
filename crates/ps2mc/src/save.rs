//! A single save: one directory and its files, independent of where it
//! lives (card, `.psu` file, PS2 over the network ...).

use crate::dirent::Mode;
use crate::iconsys::IconSys;
use crate::tod::Tod;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveFile {
    pub name: String,
    pub mode: Mode,
    pub created: Tod,
    pub modified: Tod,
    pub data: Vec<u8>,
}

impl SaveFile {
    pub fn new(name: &str, data: Vec<u8>) -> SaveFile {
        let now = Tod::now();
        SaveFile { name: name.into(), mode: Mode::save_file(), created: now, modified: now, data }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Save {
    /// Directory name, e.g. `BASLUS-20312SAVE00`.
    pub name: String,
    pub mode: Mode,
    pub created: Tod,
    pub modified: Tod,
    pub files: Vec<SaveFile>,
}

impl Save {
    pub fn new(name: &str) -> Save {
        let now = Tod::now();
        Save { name: name.into(), mode: Mode::save_dir(), created: now, modified: now, files: Vec::new() }
    }

    pub fn file(&self, name: &str) -> Option<&SaveFile> {
        self.files.iter().find(|f| f.name == name)
    }

    pub fn icon_sys(&self) -> Option<IconSys> {
        self.file("icon.sys").and_then(|f| IconSys::parse(&f.data).ok())
    }

    /// Title from `icon.sys`, or an empty string.
    pub fn title(&self) -> String {
        self.icon_sys().map(|i| i.title()).unwrap_or_default()
    }

    /// Sum of the file sizes.
    pub fn data_size(&self) -> u64 {
        self.files.iter().map(|f| f.data.len() as u64).sum()
    }

    /// Size on a standard card (1 KB clusters), as the PS2 browser counts it.
    pub fn card_size_kb(&self) -> u64 {
        ((self.files.len() + 2).div_ceil(2)
            + self.files.iter().map(|f| f.data.len().div_ceil(1024)).sum::<usize>()) as u64
    }

    /// Product code guessed from the directory name (e.g. `SLUS-20312`).
    pub fn game_id(&self) -> Option<String> {
        game_id_from_dir(&self.name)
    }

    /// Region from the second character of the directory name.
    pub fn region(&self) -> &'static str {
        region_from_dir(&self.name)
    }

    /// CRC32 over all file contents; handy to spot duplicates.
    pub fn content_crc(&self) -> u32 {
        let mut h = crc32fast::Hasher::new();
        for f in &self.files {
            h.update(f.name.as_bytes());
            h.update(&f.data);
        }
        h.finalize()
    }

    /// Default file name for an exported copy.
    pub fn suggested_filename(&self, ext: &str) -> String {
        let base: String = self
            .name
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || "-_.".contains(c) { c } else { '_' })
            .collect();
        format!("{base}.{ext}")
    }
}

/// Product code from a save directory name, as used for GameID folders
/// on the MemCard PRO2: `BASLUS-20312SAVE` -> `SLUS-20312`. Also accepts
/// names without the dash (`BASLUS20312...`).
pub fn game_id_from_dir(name: &str) -> Option<String> {
    let b = name.as_bytes();
    if b.len() < 11 || b[0] != b'B' || !b[2..6].iter().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    let prefix = name[2..6].to_ascii_uppercase();
    let digits: &str = if b[6] == b'-' || b[6] == b'_' {
        name.get(7..12)?
    } else {
        name.get(6..11)?
    };
    if digits.len() == 5 && digits.chars().all(|c| c.is_ascii_digit()) {
        Some(format!("{prefix}-{digits}"))
    } else {
        None
    }
}

pub fn region_from_dir(name: &str) -> &'static str {
    let b = name.as_bytes();
    if b.len() < 2 || b[0] != b'B' {
        return "";
    }
    match b[1] {
        b'A' => "NTSC-U",
        b'E' => "PAL",
        b'I' => "NTSC-J",
        b'K' => "NTSC-K",
        b'C' => "NTSC-C",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ids() {
        assert_eq!(game_id_from_dir("BASLUS-20312SAVE00").as_deref(), Some("SLUS-20312"));
        assert_eq!(game_id_from_dir("BESCES-50000"), Some("SCES-50000".into()));
        assert_eq!(game_id_from_dir("BADATA-SYSTEM"), None);
        assert_eq!(region_from_dir("BISLPM-65555"), "NTSC-J");
        assert_eq!(game_id_from_dir("BASLUS20312x").as_deref(), Some("SLUS-20312"));
        assert_eq!(game_id_from_dir("BASCUS-94163FF7-S01").as_deref(), Some("SCUS-94163"));
    }
}
