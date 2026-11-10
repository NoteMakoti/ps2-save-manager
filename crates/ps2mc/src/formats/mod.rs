//! Single-save exchange formats.

pub mod cbs;
pub mod lzari;
pub mod max;
pub mod psu;
pub mod sps;

use crate::error::{Error, Result};
use crate::save::Save;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SaveFormat {
    /// EMS / uLaunchELF / OPL `.psu`
    Psu,
    /// Action Replay MAX `.max`
    Max,
    /// CodeBreaker `.cbs`
    Cbs,
    /// SharkPort / X-Port `.sps` / `.xps`
    Sps,
}

impl SaveFormat {
    pub const ALL: [SaveFormat; 4] = [SaveFormat::Psu, SaveFormat::Max, SaveFormat::Cbs, SaveFormat::Sps];

    pub fn extension(self) -> &'static str {
        match self {
            SaveFormat::Psu => "psu",
            SaveFormat::Max => "max",
            SaveFormat::Cbs => "cbs",
            SaveFormat::Sps => "sps",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            SaveFormat::Psu => "EMS / uLaunchELF (.psu)",
            SaveFormat::Max => "Action Replay MAX (.max)",
            SaveFormat::Cbs => "CodeBreaker (.cbs)",
            SaveFormat::Sps => "SharkPort / X-Port (.sps/.xps)",
        }
    }

    pub fn from_extension(ext: &str) -> Option<SaveFormat> {
        match ext.to_ascii_lowercase().as_str() {
            "psu" => Some(SaveFormat::Psu),
            "max" => Some(SaveFormat::Max),
            "cbs" => Some(SaveFormat::Cbs),
            "sps" | "xps" => Some(SaveFormat::Sps),
            _ => None,
        }
    }

    pub fn from_path(path: &std::path::Path) -> Option<SaveFormat> {
        path.extension().and_then(|e| e.to_str()).and_then(SaveFormat::from_extension)
    }

    /// Sniff the format from file contents.
    pub fn detect(data: &[u8]) -> Option<SaveFormat> {
        if data.starts_with(max::MAGIC) {
            Some(SaveFormat::Max)
        } else if data.starts_with(cbs::MAGIC) {
            Some(SaveFormat::Cbs)
        } else if data.starts_with(sps::MAGIC) {
            Some(SaveFormat::Sps)
        } else if psu::looks_like(data) {
            Some(SaveFormat::Psu)
        } else {
            None
        }
    }

    pub fn load(self, data: &[u8]) -> Result<Save> {
        match self {
            SaveFormat::Psu => psu::load(data),
            SaveFormat::Max => max::load(data),
            SaveFormat::Cbs => cbs::load(data),
            SaveFormat::Sps => sps::load(data),
        }
    }

    pub fn save(self, save: &Save) -> Result<Vec<u8>> {
        match self {
            SaveFormat::Psu => Ok(psu::save(save)),
            SaveFormat::Max => Ok(max::save(save)),
            SaveFormat::Cbs => cbs::save(save),
            SaveFormat::Sps => Ok(sps::save(save)),
        }
    }
}

impl std::fmt::Display for SaveFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.description())
    }
}

/// Load a save, detecting the format from the content.
pub fn load_any(data: &[u8]) -> Result<(SaveFormat, Save)> {
    let fmt = SaveFormat::detect(data).ok_or(Error::UnknownFormat)?;
    Ok((fmt, fmt.load(data)?))
}

/// Load a save file from disk.
pub fn load_file(path: impl AsRef<std::path::Path>) -> Result<(SaveFormat, Save)> {
    let data = std::fs::read(path)?;
    load_any(&data)
}

// small helpers shared by the formats
pub(crate) struct Reader<'a> {
    pub data: &'a [u8],
    pub pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Reader { data, pos: 0 }
    }
    pub fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.pos + n > self.data.len() {
            return Err(Error::CorruptSave("unexpected end of file".into()));
        }
        let s = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }
    pub fn u16(&mut self) -> Result<u16> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }
    pub fn u32(&mut self) -> Result<u32> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    pub fn rest(&self) -> &'a [u8] {
        &self.data[self.pos..]
    }
}

pub fn cstr(b: &[u8]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    b[..end].iter().map(|&c| c as char).collect()
}

pub(crate) fn put_fixed(out: &mut Vec<u8>, s: &str, len: usize) {
    let mut b: Vec<u8> = s.chars().map(|c| c as u32 as u8).take(len).collect();
    b.resize(len, 0);
    out.extend_from_slice(&b);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::save::{Save, SaveFile};

    pub fn sample_save() -> Save {
        let mut s = Save::new("BASLUS-12345SAVE");
        let mut icon = vec![0u8; 964];
        icon[..4].copy_from_slice(b"PS2D");
        let title = "Test Game\u{3000}Slot 1";
        let (enc, _, _) = encoding_rs::SHIFT_JIS.encode(title);
        icon[0xC0..0xC0 + enc.len()].copy_from_slice(&enc);
        icon[6..8].copy_from_slice(&(9u16).to_le_bytes());
        icon[0x104..0x10C].copy_from_slice(b"icon.ico");
        s.files.push(SaveFile::new("icon.sys", icon));
        s.files.push(SaveFile::new("icon.ico", (0..3000u32).map(|i| (i * 13) as u8).collect()));
        s.files.push(SaveFile::new("BASLUS-12345SAVE", vec![0x42; 12345]));
        s.files.push(SaveFile::new("empty", vec![]));
        s
    }

    fn same_content(a: &Save, b: &Save) {
        assert_eq!(a.name, b.name);
        assert_eq!(a.files.len(), b.files.len());
        for (x, y) in a.files.iter().zip(&b.files) {
            assert_eq!(x.name, y.name);
            assert_eq!(x.data, y.data, "file {}", x.name);
        }
    }

    #[test]
    fn all_formats_roundtrip() {
        let s = sample_save();
        assert_eq!(s.title(), "Test Game Slot 1");
        for fmt in SaveFormat::ALL {
            let bytes = fmt.save(&s).unwrap();
            assert_eq!(SaveFormat::detect(&bytes), Some(fmt), "{fmt:?}");
            let back = fmt.load(&bytes).unwrap();
            same_content(&s, &back);
        }
    }
}
