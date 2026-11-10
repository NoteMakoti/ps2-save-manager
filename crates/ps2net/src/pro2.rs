//! Exporting to a MemCard PRO2: whole cards, and one card per game.
//!
//! SD card layout used by the PRO2 (and SD2PSX-style devices):
//!
//! ```text
//! /PS2/<GameID>/<GameID>-<channel>.mc2     8 MB card with ECC
//! /PS2/<GameID>/name.txt                   display name
//! /PS1/<GameID>/<GameID>-<channel>.mcd     128 KB PS1 card
//! /PS2/<CardName>/<CardName>-<channel>.mc2 named (non-GameID) cards
//! ```
//!
//! GameIDs are product codes like `SLUS-20312`, taken from the save's
//! directory name (`BASLUS-20312…`). The PRO2 picks the per-game card
//! from the disc it detects, so a game whose saves use a different code
//! than its disc (rare: some multi-disc or budget re-releases) will need
//! its folder renamed on the SD card.

use ps2mc::anysave::{GameGroups, Platform};
use ps2mc::{AnySave, MemoryCard, Ps1Card};

use crate::memcardpro::MemcardPro;
use crate::{NetError, NetResult};

/// `/PS2/SLUS-20312/SLUS-20312-1.mc2`
pub fn card_path(p: Platform, folder: &str, channel: u32) -> String {
    format!("/{}/{folder}/{folder}-{channel}.{}", p.folder(), p.card_ext())
}

/// Folder names we are willing to create on the SD card.
pub fn valid_folder(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name != "."
        && name != ".."
        && name.chars().all(|c| c.is_ascii_alphanumeric() || " -_.()".contains(c))
}

fn parent(path: &str) -> &str {
    &path[..path.rfind('/').unwrap_or(0)]
}

impl MemcardPro {
    /// Does a remote file exist? (Looks it up in its parent directory.)
    pub fn exists(&mut self, path: &str) -> NetResult<bool> {
        let (dir, name) = path.rsplit_once('/').unwrap_or(("/", path));
        let dir = if dir.is_empty() { "/" } else { dir };
        match self.list(dir) {
            Ok(entries) => Ok(entries.iter().any(|e| e.name == name)),
            Err(NetError::Ftp(_)) => Ok(false), // parent missing
            Err(e) => Err(e),
        }
    }

    /// Create every directory leading up to `path` (a file path).
    pub fn ensure_parent_dirs(&mut self, path: &str) -> NetResult<()> {
        let mut cur = String::new();
        for part in parent(path).split('/').filter(|p| !p.is_empty()) {
            cur.push('/');
            cur.push_str(part);
            if !self.exists(&cur)? {
                self.mkdir(&cur)?;
            }
        }
        Ok(())
    }
}

/// Upload a whole card image as `/<PS1|PS2>/<folder>/<folder>-<channel>.<ext>`.
/// An existing card is only replaced when `replace` is set, and is then
/// kept as `.bak`. Returns the remote path.
pub fn export_whole_card(
    m: &mut MemcardPro,
    platform: Platform,
    folder: &str,
    channel: u32,
    image: &[u8],
    replace: bool,
) -> NetResult<String> {
    if !valid_folder(folder) {
        return Err(NetError::Refused(format!("'{folder}' is not a usable folder name")));
    }
    if !(1..=8).contains(&channel) {
        return Err(NetError::Refused("channel must be 1–8".into()));
    }
    // sanity check what we are about to put on the device
    match platform {
        Platform::Ps2 => {
            let c = MemoryCard::open(image.to_vec())?;
            if !c.has_ecc() {
                return Err(NetError::Refused("PS2 cards for the MemCard PRO2 must include ECC".into()));
            }
        }
        Platform::Ps1 => {
            Ps1Card::open(image.to_vec())?;
        }
    }
    let path = card_path(platform, folder, channel);
    let exists = m.exists(&path)?;
    if exists && !replace {
        return Err(NetError::Refused(format!("{path} already exists on the MemCard PRO2 (tick \"Replace\" to overwrite it)")));
    }
    m.ensure_parent_dirs(&path)?;
    m.upload(&path, image, exists)?;
    Ok(path)
}

#[derive(Debug, Default, Clone)]
pub struct PerGameReport {
    /// New cards written (remote paths).
    pub created: Vec<String>,
    /// Existing cards that saves were merged into.
    pub updated: Vec<String>,
    /// Saves that were already on the target card (not overwritten).
    pub kept: Vec<String>,
    /// Saves with no product code in their name.
    pub unmatched: Vec<String>,
    /// (game id, error)
    pub failed: Vec<(String, String)>,
}

impl PerGameReport {
    pub fn summary(&self) -> String {
        let mut s = format!("{} new card(s), {} updated", self.created.len(), self.updated.len());
        if !self.kept.is_empty() {
            s += &format!(", {} save(s) already present", self.kept.len());
        }
        if !self.unmatched.is_empty() {
            s += &format!(", {} without a game ID skipped", self.unmatched.len());
        }
        if !self.failed.is_empty() {
            s += &format!(", {} failed", self.failed.len());
        }
        s
    }
}

/// Write each game's saves to its own GameID card. Existing cards are
/// downloaded, the saves merged in (replacing same-named saves only when
/// `overwrite`), and uploaded back with a `.bak` of the previous version.
pub fn export_per_game(
    m: &mut MemcardPro,
    groups: &GameGroups,
    channel: u32,
    overwrite: bool,
    write_names: bool,
    mut progress: impl FnMut(usize, usize, &str),
) -> NetResult<PerGameReport> {
    let mut rep = PerGameReport { unmatched: groups.unmatched.iter().map(|s| s.name().to_string()).collect(), ..Default::default() };
    let total = groups.games.len();
    for (n, ((platform, id), saves)) in groups.games.iter().enumerate() {
        progress(n, total, id);
        let path = card_path(*platform, id, channel);
        let r = (|| -> NetResult<()> {
            let exists = m.exists(&path)?;
            let mut added = 0;
            let image = match platform {
                Platform::Ps2 => {
                    let mut card = if exists { MemoryCard::open(m.download(&path)?)? } else { MemoryCard::format(8, true)? };
                    for s in saves {
                        if let AnySave::Ps2(s) = s {
                            match card.import_save(s, overwrite) {
                                Ok(()) => added += 1,
                                Err(ps2mc::Error::Exists(_)) => rep.kept.push(s.name.clone()),
                                Err(e) => return Err(e.into()),
                            }
                        }
                    }
                    card.to_bytes_with_ecc(true)
                }
                Platform::Ps1 => {
                    let mut card = if exists { Ps1Card::open(m.download(&path)?)? } else { Ps1Card::format() };
                    for s in saves {
                        if let AnySave::Ps1(s) = s {
                            match card.import_save(s, overwrite) {
                                Ok(()) => added += 1,
                                Err(ps2mc::Error::Exists(_)) => rep.kept.push(s.name.clone()),
                                Err(e) => return Err(e.into()),
                            }
                        }
                    }
                    card.as_bytes().to_vec()
                }
            };
            if added == 0 {
                return Ok(()); // everything was already there; leave the card alone
            }
            m.ensure_parent_dirs(&path)?;
            m.upload(&path, &image, exists)?;
            if write_names {
                let name_txt = format!("{}/name.txt", parent(&path));
                let title = saves.iter().map(|s| s.title()).find(|t| !t.is_empty());
                if let Some(t) = title {
                    if !m.exists(&name_txt)? {
                        m.upload(&name_txt, t.as_bytes(), false)?;
                    }
                }
            }
            if exists {
                rep.updated.push(path.clone());
            } else {
                rep.created.push(path.clone());
            }
            Ok(())
        })();
        if let Err(e) = r {
            rep.failed.push((id.clone(), e.to_string()));
        }
    }
    progress(total, total, "");
    Ok(rep)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn paths() {
        assert_eq!(card_path(Platform::Ps2, "SLUS-20312", 1), "/PS2/SLUS-20312/SLUS-20312-1.mc2");
        assert_eq!(card_path(Platform::Ps1, "SCUS-94163", 2), "/PS1/SCUS-94163/SCUS-94163-2.mcd");
        assert!(valid_folder("MemoryCard2"));
        assert!(!valid_folder("../x"));
    }
}
