//! Save sources the GUI can show side by side: card images, a PS2 over
//! the network, and a folder of save files (local or a mounted SMB share).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, bail, Context, Result};
use ps2mc::dirent::valid_name;
use ps2mc::anysave::{self, AnySave, Platform};
use ps2mc::{formats, MemoryCard, Ps1Card, Ps1Save, Save, SaveFile, SaveFormat, Tod};
use ps2net::client::{MmceMode, MmceTarget};
use ps2net::Ps2Client;

/// Where a card image came from on a MemCard PRO2, so it can be sent back.
#[derive(Clone, Debug)]
pub struct McpOrigin {
    pub host: String,
    pub user: String,
    pub password: String,
    pub remote_path: String,
}

#[allow(dead_code)]
pub enum Backend {
    Card { card: MemoryCard, path: Option<PathBuf>, mcp: Option<McpOrigin> },
    Ps1 { card: Ps1Card, path: Option<PathBuf>, mcp: Option<McpOrigin> },
    Ps2 { client: Arc<Mutex<Ps2Client>>, port: u8, host: String },
    Library { dir: PathBuf, format: SaveFormat },
}

#[derive(Clone, Debug)]
pub struct SaveRow {
    /// Directory name for cards / PS2, file path for libraries.
    pub key: String,
    pub name: String,
    pub title: String,
    pub size_kb: u64,
    pub modified: String,
    pub format: String,
}

#[derive(Clone, Debug, Default)]
pub struct Status {
    pub free_kb: Option<u64>,
    pub dirty: bool,
    pub note: String,
}

const SAVE_EXTS: [&str; 6] = ["psu", "max", "cbs", "sps", "xps", "mcs"];

const PS1_ONLY: &str = "PS1 saves can only go on a PS1 card (.mcd) or into a save folder";
const PS2_ONLY: &str = "PS2 saves can't go on a PS1 card";

impl Backend {
    pub fn kind(&self) -> &'static str {
        match self {
            Backend::Card { mcp: Some(_), .. } => "MemCard PRO2",
            Backend::Card { .. } => "Card image",
            Backend::Ps1 { mcp: Some(_), .. } => "MemCard PRO2 (PS1)",
            Backend::Ps1 { .. } => "PS1 card",
            Backend::Ps2 { .. } => "PS2",
            Backend::Library { .. } => "Folder",
        }
    }

    pub fn list(&mut self) -> Result<(Vec<SaveRow>, Status)> {
        match self {
            Backend::Card { card, .. } => {
                let rows = card
                    .saves()?
                    .into_iter()
                    .map(|s| SaveRow {
                        key: s.name.clone(),
                        name: s.name,
                        title: s.title,
                        size_kb: s.size.div_ceil(1024),
                        modified: s.modified.to_string(),
                        format: String::new(),
                    })
                    .collect();
                let info = card.info()?;
                Ok((rows, Status { free_kb: Some(info.free_bytes / 1024), dirty: card.is_dirty(), note: String::new() }))
            }
            Backend::Ps1 { card, .. } => {
                let rows = card
                    .saves()
                    .into_iter()
                    .map(|s| SaveRow {
                        key: s.name.clone(),
                        name: s.name,
                        title: s.title,
                        size_kb: s.blocks as u64 * 8,
                        modified: String::new(),
                        format: "PS1".into(),
                    })
                    .collect();
                let free = card.free_blocks();
                Ok((rows, Status { free_kb: Some(free as u64 * 8), dirty: card.is_dirty(), note: format!("{free} of 15 blocks free") }))
            }
            Backend::Ps2 { client, port, .. } => {
                let mut c = client.lock().unwrap();
                let info = c.card_info(*port)?;
                if !info.present {
                    return Ok((vec![], Status { note: "no memory card in this slot".into(), ..Default::default() }));
                }
                if info.card_type != 2 {
                    return Ok((vec![], Status { note: format!("card type {} is not a PS2 card", info.card_type), ..Default::default() }));
                }
                if !info.formatted {
                    return Ok((vec![], Status { note: "card is not formatted".into(), ..Default::default() }));
                }
                let rows = c
                    .list_saves(*port)?
                    .into_iter()
                    .map(|s| SaveRow {
                        key: s.name.clone(),
                        name: s.name,
                        title: s.title,
                        size_kb: (s.size as u64).div_ceil(1024),
                        modified: s.modified.to_string(),
                        format: String::new(),
                    })
                    .collect();
                Ok((rows, Status { free_kb: Some(info.free_kb as u64), dirty: false, note: String::new() }))
            }
            Backend::Library { dir, .. } => {
                let mut rows = Vec::new();
                let mut bad = 0;
                for e in std::fs::read_dir(&*dir).with_context(|| format!("reading {}", dir.display()))? {
                    let p = e?.path();
                    let ext = p.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).unwrap_or_default();
                    if !SAVE_EXTS.contains(&ext.as_str()) {
                        continue;
                    }
                    let data = match std::fs::read(&p) {
                        Ok(d) => d,
                        Err(_) => {
                            bad += 1;
                            continue;
                        }
                    };
                    match anysave::load_any(&data) {
                        Ok(AnySave::Ps2(s)) => rows.push(SaveRow {
                            key: p.to_string_lossy().into_owned(),
                            name: s.name.clone(),
                            title: s.title(),
                            size_kb: s.card_size_kb(),
                            modified: s.modified.to_string(),
                            format: SaveFormat::detect(&data).map(|f| f.extension()).unwrap_or("").to_string(),
                        }),
                        Ok(AnySave::Ps1(s)) => rows.push(SaveRow {
                            key: p.to_string_lossy().into_owned(),
                            name: s.name.clone(),
                            title: s.title(),
                            size_kb: s.blocks() as u64 * 8,
                            modified: String::new(),
                            format: "mcs".into(),
                        }),
                        Err(_) => bad += 1,
                    }
                }
                rows.sort_by(|a, b| a.name.cmp(&b.name));
                let note = if bad > 0 { format!("{bad} unreadable file(s) skipped") } else { String::new() };
                Ok((rows, Status { note, ..Default::default() }))
            }
        }
    }

    /// PS2 save with its files (used by the file explorer).
    pub fn get(&mut self, key: &str) -> Result<Save> {
        match self.get_any(key)? {
            AnySave::Ps2(s) => Ok(s),
            AnySave::Ps1(_) => bail!("PS1 saves are a single block of data, there are no files inside to explore"),
        }
    }

    /// Any save, PS1 or PS2.
    pub fn get_any(&mut self, key: &str) -> Result<AnySave> {
        Ok(match self {
            Backend::Card { card, .. } => AnySave::Ps2(card.export_save(key)?),
            Backend::Ps1 { card, .. } => AnySave::Ps1(card.export_save(key)?),
            Backend::Ps2 { client, port, .. } => AnySave::Ps2(client.lock().unwrap().get_save(*port, key)?),
            Backend::Library { .. } => anysave::load_file(key)?,
        })
    }

    pub fn put_any(&mut self, save: &AnySave, overwrite: bool) -> Result<()> {
        match (self, save) {
            (Backend::Ps1 { card, .. }, AnySave::Ps1(s)) => card.import_save(s, overwrite)?,
            (Backend::Ps1 { .. }, AnySave::Ps2(_)) => bail!(PS2_ONLY),
            (Backend::Library { dir, .. }, AnySave::Ps1(s)) => {
                let p = dir.join(s.suggested_filename("mcs"));
                if p.exists() && !overwrite {
                    bail!("{} already exists in the folder", p.display());
                }
                std::fs::write(&p, s.to_mcs())?;
            }
            (_, AnySave::Ps1(_)) => bail!(PS1_ONLY),
            (b, AnySave::Ps2(s)) => b.put(s, overwrite)?,
        }
        Ok(())
    }

    pub fn put(&mut self, save: &Save, overwrite: bool) -> Result<()> {
        match self {
            Backend::Card { card, .. } => card.import_save(save, overwrite)?,
            Backend::Ps1 { .. } => bail!(PS2_ONLY),
            Backend::Ps2 { client, port, .. } => client.lock().unwrap().put_save(*port, save, overwrite)?,
            Backend::Library { dir, format } => {
                let p = dir.join(save.suggested_filename(format.extension()));
                if p.exists() && !overwrite {
                    bail!("{} already exists in the folder", p.display());
                }
                std::fs::write(&p, format.save(save)?)?;
            }
        }
        Ok(())
    }

    pub fn delete(&mut self, key: &str) -> Result<()> {
        match self {
            Backend::Card { card, .. } => card.remove_all(key)?,
            Backend::Ps1 { card, .. } => card.remove(key)?,
            Backend::Ps2 { client, port, .. } => client.lock().unwrap().delete_save(*port, key)?,
            Backend::Library { .. } => std::fs::remove_file(key)?,
        }
        Ok(())
    }

    pub fn rename(&mut self, key: &str, new_name: &str) -> Result<()> {
        match self {
            Backend::Card { card, .. } => Ok(card.rename(key, new_name)?),
            Backend::Ps1 { card, .. } => {
                let mut s = card.export_save(key)?;
                s.name = new_name.to_string();
                card.import_save(&s, false)?;
                card.remove(key)?;
                Ok(())
            }
            Backend::Ps2 { client, port, .. } => {
                // no rename on the wire: copy then delete
                let mut c = client.lock().unwrap();
                let mut s = c.get_save(*port, key)?;
                s.name = new_name.to_string();
                c.put_save(*port, &s, false)?;
                c.delete_save(*port, key)?;
                Ok(())
            }
            Backend::Library { .. } => {
                let data = std::fs::read(key)?;
                if ps2mc::ps1::is_mcs(&data) {
                    let mut s = Ps1Save::from_mcs(&data)?;
                    s.name = new_name.to_string();
                    std::fs::write(key, s.to_mcs())?;
                    return Ok(());
                }
                let (fmt, mut s) = formats::load_any(&data)?;
                s.name = new_name.to_string();
                std::fs::write(key, fmt.save(&s)?)?;
                Ok(())
            }
        }
    }

    /// The whole card as an image for the MemCard PRO2 (PS2 cards with
    /// ECC). A save folder becomes a new card holding all of its saves.
    pub fn card_image(&mut self, mut progress: impl FnMut(String)) -> Result<(Platform, Vec<u8>)> {
        match self {
            Backend::Card { card, .. } => Ok((Platform::Ps2, card.to_bytes_with_ecc(true))),
            Backend::Ps1 { card, .. } => Ok((Platform::Ps1, card.as_bytes().to_vec())),
            Backend::Ps2 { client, port, .. } => {
                let img = client.lock().unwrap().dump_to_image(*port, true, |i, n, name| {
                    if !name.is_empty() {
                        progress(format!("reading {name} from the PS2 ({}/{n})", i + 1));
                    }
                })?;
                Ok((Platform::Ps2, img.into_bytes()))
            }
            Backend::Library { dir, .. } => {
                let mut ps2 = MemoryCard::format(8, true)?;
                let mut ps1 = Ps1Card::format();
                let (mut n2, mut n1) = (0, 0);
                for e in std::fs::read_dir(&*dir)? {
                    let p = e?.path();
                    match anysave::load_file(&p) {
                        Ok(AnySave::Ps2(s)) => {
                            ps2.import_save(&s, false).with_context(|| format!("{} does not fit on one card", s.name))?;
                            n2 += 1;
                        }
                        Ok(AnySave::Ps1(s)) => {
                            ps1.import_save(&s, false).with_context(|| format!("{} does not fit on one PS1 card", s.name))?;
                            n1 += 1;
                        }
                        Err(_) => {}
                    }
                }
                if n2 == 0 && n1 == 0 {
                    bail!("no saves in this folder");
                }
                if n2 > 0 && n1 > 0 {
                    bail!("this folder has both PS1 and PS2 saves; use per-game export instead");
                }
                Ok(if n2 > 0 { (Platform::Ps2, ps2.into_bytes()) } else { (Platform::Ps1, ps1.as_bytes().to_vec()) })
            }
        }
    }

    // -----------------------------------------------------------------
    // Files inside one save folder
    // -----------------------------------------------------------------

    /// Add or replace files in the save `key`. Fails up front (changing
    /// nothing) if a name is invalid, or exists and `replace` is off.
    pub fn put_files(&mut self, key: &str, files: &[(String, Vec<u8>)], replace: bool) -> Result<()> {
        for (name, _) in files {
            if !valid_name(name) {
                bail!("'{name}' is not a valid memory card file name (max 31 ASCII characters, no / \\ ? *)");
            }
        }
        let existing: Vec<String> = self.get(key)?.files.into_iter().map(|f| f.name).collect();
        if !replace {
            let clash: Vec<&str> = files.iter().map(|(n, _)| n.as_str()).filter(|n| existing.iter().any(|e| e == n)).collect();
            if !clash.is_empty() {
                bail!("already in {key}: {} (tick \"Replace existing files\")", clash.join(", "));
            }
        }
        match self {
            Backend::Ps1 { .. } => bail!("PS1 saves have no files inside them"),
            Backend::Card { card, .. } => {
                for (name, data) in files {
                    card.write_file(&format!("{key}/{name}"), data)?;
                }
            }
            Backend::Ps2 { client, port, .. } => {
                let mut c = client.lock().unwrap();
                for (name, data) in files {
                    c.write_file(*port, &format!("{key}/{name}"), data)?;
                }
            }
            Backend::Library { .. } => {
                let (fmt, mut s) = formats::load_file(key)?;
                let now = Tod::now();
                for (name, data) in files {
                    match s.files.iter_mut().find(|f| &f.name == name) {
                        Some(f) => {
                            f.data = data.clone();
                            f.modified = now;
                        }
                        None => s.files.push(SaveFile::new(name, data.clone())),
                    }
                }
                s.modified = now;
                std::fs::write(key, fmt.save(&s)?)?;
            }
        }
        Ok(())
    }

    pub fn delete_files(&mut self, key: &str, names: &[String]) -> Result<()> {
        match self {
            Backend::Ps1 { .. } => bail!("PS1 saves have no files inside them"),
            Backend::Card { card, .. } => {
                for n in names {
                    card.remove(&format!("{key}/{n}"))?;
                }
            }
            Backend::Ps2 { client, port, .. } => {
                let mut c = client.lock().unwrap();
                for n in names {
                    c.delete(*port, &format!("{key}/{n}"))?;
                }
            }
            Backend::Library { .. } => {
                let (fmt, mut s) = formats::load_file(key)?;
                s.files.retain(|f| !names.contains(&f.name));
                std::fs::write(key, fmt.save(&s)?)?;
            }
        }
        Ok(())
    }

    pub fn rename_file(&mut self, key: &str, from: &str, to: &str) -> Result<()> {
        if !valid_name(to) {
            bail!("'{to}' is not a valid memory card file name");
        }
        match self {
            Backend::Ps1 { .. } => bail!("PS1 saves have no files inside them"),
            Backend::Card { card, .. } => card.rename(&format!("{key}/{from}"), to)?,
            Backend::Ps2 { client, port, .. } => {
                // no rename on the wire: write the new name, then delete the old
                let mut c = client.lock().unwrap();
                let s = c.get_save(*port, key)?;
                if s.file(to).is_some() {
                    bail!("{to} already exists");
                }
                let f = s.file(from).ok_or_else(|| anyhow!("{from} not found"))?;
                c.write_file(*port, &format!("{key}/{to}"), &f.data)?;
                c.delete(*port, &format!("{key}/{from}"))?;
            }
            Backend::Library { .. } => {
                let (fmt, mut s) = formats::load_file(key)?;
                if s.file(to).is_some() {
                    bail!("{to} already exists");
                }
                let f = s.files.iter_mut().find(|f| f.name == from).ok_or_else(|| anyhow!("{from} not found"))?;
                f.name = to.to_string();
                std::fs::write(key, fmt.save(&s)?)?;
            }
        }
        Ok(())
    }

    pub fn save_card(&mut self, to: Option<&Path>) -> Result<PathBuf> {
        match self {
            Backend::Ps1 { card, path, .. } => {
                let target = to.map(Path::to_path_buf).or_else(|| path.clone()).ok_or_else(|| anyhow!("no file name"))?;
                card.save_to(&target)?;
                *path = Some(target.clone());
                Ok(target)
            }
            Backend::Card { card, path, .. } => {
                let target = to.map(Path::to_path_buf).or_else(|| path.clone()).ok_or_else(|| anyhow!("no file name"))?;
                card.save_to(&target)?;
                *path = Some(target.clone());
                Ok(target)
            }
            _ => bail!("not a card image"),
        }
    }

    pub fn mmce(&mut self, target: MmceTarget, mode: MmceMode) -> Result<String> {
        match self {
            Backend::Ps2 { client, port, .. } => {
                let mut c = client.lock().unwrap();
                c.mmce_set(*port, target, mode)?;
                drop(c);
                std::thread::sleep(std::time::Duration::from_millis(1200));
                let mut c = client.lock().unwrap();
                let s = c.mmce_status(*port)?;
                Ok(format!("card {} · channel {} {}", s.card, s.channel, s.game_id))
            }
            _ => bail!("not a PS2 source"),
        }
    }

    pub fn mmce_status(&mut self) -> Result<String> {
        match self {
            Backend::Ps2 { client, port, .. } => {
                let s = client.lock().unwrap().mmce_status(*port)?;
                Ok(format!("card {} · channel {} {}", s.card, s.channel, s.game_id))
            }
            _ => bail!("not a PS2 source"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Save {
        let mut s = Save::new("BASLUS-12345");
        s.files.push(SaveFile::new("data", vec![1; 3000]));
        s
    }

    fn exercise(b: &mut Backend, key: &str) {
        b.put_files(key, &[("extra.bin".into(), vec![9; 1500])], false).unwrap();
        assert!(b.put_files(key, &[("extra.bin".into(), vec![0])], false).is_err());
        assert!(b.put_files(key, &[("bad/name".into(), vec![0])], true).is_err());
        b.put_files(key, &[("extra.bin".into(), vec![7; 10])], true).unwrap();
        assert_eq!(b.get(key).unwrap().file("extra.bin").unwrap().data, vec![7; 10]);
        b.rename_file(key, "extra.bin", "renamed.bin").unwrap();
        assert!(b.rename_file(key, "renamed.bin", "data").is_err());
        b.delete_files(key, &["renamed.bin".into()]).unwrap();
        let s = b.get(key).unwrap();
        assert_eq!(s.files.len(), 1);
        assert_eq!(s.files[0].data, vec![1; 3000]);
    }

    #[test]
    fn card_files() {
        let mut card = MemoryCard::format(8, true).unwrap();
        card.import_save(&sample(), false).unwrap();
        let mut b = Backend::Card { card, path: None, mcp: None };
        exercise(&mut b, "BASLUS-12345");
        if let Backend::Card { card, .. } = &b {
            assert!(card.check().unwrap().is_empty());
        }
    }

    #[test]
    fn library_files() {
        let dir = std::env::temp_dir().join(format!("ps2sm-lib-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for fmt in SaveFormat::ALL {
            let mut b = Backend::Library { dir: dir.clone(), format: fmt };
            b.put(&sample(), true).unwrap();
            let key = dir.join(sample().suggested_filename(fmt.extension())).to_string_lossy().into_owned();
            exercise(&mut b, &key);
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn ps2_files() {
        let mut card = MemoryCard::format(8, false).unwrap();
        card.import_save(&sample(), false).unwrap();
        let slots = ps2net::server::slots(Some(ps2net::server::Slot { card, path: None }), None);
        let addr = ps2net::server::spawn_local(slots).unwrap();
        let client = Arc::new(Mutex::new(Ps2Client::connect(&addr).unwrap()));
        let mut b = Backend::Ps2 { client, port: 0, host: addr };
        exercise(&mut b, "BASLUS-12345");
    }

    #[test]
    fn ps1_sources() {
        let mut c = Ps1Card::format();
        let mut data = vec![0u8; 2 * ps2mc::ps1::BLOCK_SIZE];
        data[..2].copy_from_slice(b"SC");
        c.import_save(&Ps1Save { name: "BASLUS-00067DRAX00".into(), data }, false).unwrap();
        let mut p1 = Backend::Ps1 { card: c, path: None, mcp: None };
        let (rows, st) = p1.list().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(st.free_kb, Some(13 * 8));
        let s = p1.get_any("BASLUS-00067DRAX00").unwrap();
        // PS1 -> PS2 card refused, PS1 -> folder as .mcs, back into a new PS1 card
        let mut card = Backend::Card { card: MemoryCard::format(8, true).unwrap(), path: None, mcp: None };
        assert!(card.put_any(&s, false).is_err());
        let dir = std::env::temp_dir().join(format!("ps2sm-ps1-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut lib = Backend::Library { dir: dir.clone(), format: SaveFormat::Psu };
        lib.put_any(&s, false).unwrap();
        let (rows, _) = lib.list().unwrap();
        assert_eq!(rows[0].format, "mcs");
        let back = lib.get_any(&rows[0].key).unwrap();
        let mut p2 = Backend::Ps1 { card: Ps1Card::format(), path: None, mcp: None };
        p2.put_any(&back, false).unwrap();
        assert_eq!(p2.get_any("BASLUS-00067DRAX00").unwrap(), s);
        let (plat, img) = lib.card_image(|_| {}).unwrap();
        assert_eq!(plat, Platform::Ps1);
        assert_eq!(img.len(), ps2mc::ps1::CARD_SIZE);
        std::fs::remove_dir_all(&dir).ok();
    }
}
