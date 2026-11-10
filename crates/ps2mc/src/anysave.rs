//! A save from either console, and grouping saves by game.

use std::collections::BTreeMap;

use crate::error::{Error, Result};
use crate::formats::SaveFormat;
use crate::ps1::{self, Ps1Save};
use crate::save::Save;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Platform {
    Ps1,
    Ps2,
}

impl Platform {
    /// Top-level folder on the MemCard PRO2 SD card.
    pub fn folder(self) -> &'static str {
        match self {
            Platform::Ps1 => "PS1",
            Platform::Ps2 => "PS2",
        }
    }
    /// Card image extension used by the MemCard PRO2.
    pub fn card_ext(self) -> &'static str {
        match self {
            Platform::Ps1 => "mcd",
            Platform::Ps2 => "mc2",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnySave {
    Ps2(Save),
    Ps1(Ps1Save),
}

impl AnySave {
    pub fn platform(&self) -> Platform {
        match self {
            AnySave::Ps2(_) => Platform::Ps2,
            AnySave::Ps1(_) => Platform::Ps1,
        }
    }
    pub fn name(&self) -> &str {
        match self {
            AnySave::Ps2(s) => &s.name,
            AnySave::Ps1(s) => &s.name,
        }
    }
    pub fn title(&self) -> String {
        match self {
            AnySave::Ps2(s) => s.title(),
            AnySave::Ps1(s) => s.title(),
        }
    }
    pub fn game_id(&self) -> Option<String> {
        crate::save::game_id_from_dir(self.name())
    }
}

/// Load any single-save file: .psu/.max/.cbs/.sps/.xps (PS2) or .mcs (PS1).
pub fn load_any(data: &[u8]) -> Result<AnySave> {
    if let Some(fmt) = SaveFormat::detect(data) {
        return Ok(AnySave::Ps2(fmt.load(data)?));
    }
    if ps1::is_mcs(data) {
        return Ok(AnySave::Ps1(Ps1Save::from_mcs(data)?));
    }
    Err(Error::UnknownFormat)
}

pub fn load_file(path: impl AsRef<std::path::Path>) -> Result<AnySave> {
    load_any(&std::fs::read(path)?)
}

/// Saves grouped per (platform, game ID). Saves without a recognisable
/// product code (system data such as `BADATA-SYSTEM`, homebrew) are
/// returned separately.
pub struct GameGroups {
    pub games: BTreeMap<(Platform, String), Vec<AnySave>>,
    pub unmatched: Vec<AnySave>,
}

pub fn group_by_game(saves: impl IntoIterator<Item = AnySave>) -> GameGroups {
    let mut games: BTreeMap<(Platform, String), Vec<AnySave>> = BTreeMap::new();
    let mut unmatched = Vec::new();
    for s in saves {
        match s.game_id() {
            Some(id) => games.entry((s.platform(), id)).or_default().push(s),
            None => unmatched.push(s),
        }
    }
    GameGroups { games, unmatched }
}

/// Suggested file name for exporting a save in its native single-file format.
pub fn export_bytes(s: &AnySave, ps2_format: SaveFormat) -> Result<(String, Vec<u8>)> {
    Ok(match s {
        AnySave::Ps2(s) => (s.suggested_filename(ps2_format.extension()), ps2_format.save(s)?),
        AnySave::Ps1(s) => (s.suggested_filename("mcs"), s.to_mcs()),
    })
}

