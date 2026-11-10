//! 512-byte directory entries, shared by the card filesystem and the
//! `.psu` save format.

use crate::tod::Tod;

pub const DIRENT_SIZE: usize = 512;

/// Directory entry mode flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Mode(pub u16);

impl Mode {
    pub const READ: u16 = 0x0001;
    pub const WRITE: u16 = 0x0002;
    pub const EXECUTE: u16 = 0x0004;
    pub const RWX: u16 = 0x0007;
    pub const PROTECTED: u16 = 0x0008;
    pub const FILE: u16 = 0x0010;
    pub const DIR: u16 = 0x0020;
    pub const O_DCREAT: u16 = 0x0040;
    pub const CLOSED: u16 = 0x0080;
    pub const F_0400: u16 = 0x0400;
    pub const POCKETSTATION: u16 = 0x0800;
    pub const PSX: u16 = 0x1000;
    pub const HIDDEN: u16 = 0x2000;
    pub const EXISTS: u16 = 0x8000;

    /// Mode for a normal save directory (0x8427).
    pub const fn save_dir() -> Mode {
        Mode(Self::EXISTS | Self::F_0400 | Self::DIR | Self::RWX)
    }
    /// Mode for a normal save file (0x8497).
    pub const fn save_file() -> Mode {
        Mode(Self::EXISTS | Self::F_0400 | Self::CLOSED | Self::FILE | Self::RWX)
    }
    pub fn exists(self) -> bool {
        self.0 & Self::EXISTS != 0
    }
    pub fn is_dir(self) -> bool {
        self.0 & (Self::FILE | Self::DIR | Self::EXISTS) == (Self::DIR | Self::EXISTS)
    }
    pub fn is_file(self) -> bool {
        self.0 & (Self::FILE | Self::DIR | Self::EXISTS) == (Self::FILE | Self::EXISTS)
    }
    pub fn is_psx(self) -> bool {
        self.0 & Self::PSX != 0
    }
    pub fn is_hidden(self) -> bool {
        self.0 & Self::HIDDEN != 0
    }
    pub fn is_protected(self) -> bool {
        self.0 & Self::PROTECTED != 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    pub mode: Mode,
    /// Bytes for files, number of entries for directories.
    pub length: u32,
    pub created: Tod,
    /// First cluster (relative to the allocatable area).
    pub cluster: u32,
    /// For "." entries: index of this directory in its parent.
    pub dir_entry: u32,
    pub modified: Tod,
    pub attr: u32,
    pub name: String,
}

impl DirEntry {
    pub fn empty() -> DirEntry {
        DirEntry {
            mode: Mode(0),
            length: 0,
            created: Tod::default(),
            cluster: 0,
            dir_entry: 0,
            modified: Tod::default(),
            attr: 0,
            name: String::new(),
        }
    }

    pub fn new(name: &str, mode: Mode, length: u32, cluster: u32, time: Tod) -> DirEntry {
        DirEntry {
            mode,
            length,
            created: time,
            cluster,
            dir_entry: 0,
            modified: time,
            attr: 0,
            name: name.to_string(),
        }
    }

    pub fn parse(b: &[u8]) -> DirEntry {
        let u32_at = |o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
        let name_raw = &b[0x40..0x60];
        let end = name_raw.iter().position(|&c| c == 0).unwrap_or(name_raw.len());
        DirEntry {
            mode: Mode(u16::from_le_bytes([b[0], b[1]])),
            length: u32_at(4),
            created: Tod::from_bytes(&b[8..16]),
            cluster: u32_at(16),
            dir_entry: u32_at(20),
            modified: Tod::from_bytes(&b[24..32]),
            attr: u32_at(32),
            name: name_raw[..end].iter().map(|&c| c as char).collect(),
        }
    }

    pub fn to_bytes(&self) -> [u8; DIRENT_SIZE] {
        let mut b = [0u8; DIRENT_SIZE];
        b[0..2].copy_from_slice(&self.mode.0.to_le_bytes());
        b[4..8].copy_from_slice(&self.length.to_le_bytes());
        b[8..16].copy_from_slice(&self.created.to_bytes());
        b[16..20].copy_from_slice(&self.cluster.to_le_bytes());
        b[20..24].copy_from_slice(&self.dir_entry.to_le_bytes());
        b[24..32].copy_from_slice(&self.modified.to_bytes());
        b[32..36].copy_from_slice(&self.attr.to_le_bytes());
        for (i, c) in self.name.chars().take(32).enumerate() {
            b[0x40 + i] = c as u32 as u8;
        }
        b
    }
}

/// Validate a file or directory name for use on a memory card.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 31
        && name != "."
        && name != ".."
        && name.chars().all(|c| (c as u32) >= 0x20 && (c as u32) < 0x7F && c != '/' && c != '\\' && c != '?' && c != '*')
}
