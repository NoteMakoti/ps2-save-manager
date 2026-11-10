//! Small helper to build or inspect cards and save files from the shell.
//!
//!   interop mkcard out.ps2          card with a few sample saves
//!   interop mksave out.max max      sample save in a given format
//!   interop readcard card.ps2       check + list saves
//!   interop dumpsave file.psu       list files with CRCs
use ps2mc::{formats, MemoryCard, Save, SaveFile, SaveFormat};

fn sample(name: &str, seed: u32) -> Save {
    let mut s = Save::new(name);
    let mut icon = vec![0u8; 964];
    icon[..4].copy_from_slice(b"PS2D");
    icon[0xC0..0xC0 + 10].copy_from_slice(b"Interop  T");
    icon[6..8].copy_from_slice(&5u16.to_le_bytes());
    s.files.push(SaveFile::new("icon.sys", icon));
    let mut x = seed;
    let noise: Vec<u8> = (0..20000)
        .map(|i| {
            x = x.wrapping_mul(1664525).wrapping_add(1013904223);
            if i % 3 == 0 { (x >> 24) as u8 } else { (i % 7) as u8 }
        })
        .collect();
    s.files.push(SaveFile::new("data", noise));
    s.files.push(SaveFile::new("small", b"tiny".to_vec()));
    s
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    match a.get(1).map(String::as_str) {
        Some("mkcard") => {
            let mut c = MemoryCard::format(8, true).unwrap();
            for i in 0..5 {
                c.import_save(&sample(&format!("BASLUS-0000{i}"), i), false).unwrap();
            }
            c.remove_all("BASLUS-00002").unwrap();
            std::fs::write(&a[2], c.as_bytes()).unwrap();
        }
        Some("mksave") => {
            let fmt = SaveFormat::from_extension(&a[3]).unwrap();
            std::fs::write(&a[2], fmt.save(&sample("BESLES-99999", 7)).unwrap()).unwrap();
        }
        Some("readcard") => {
            let c = MemoryCard::open_file(&a[2]).unwrap();
            println!("problems: {:?}", c.check().unwrap());
            for s in c.saves().unwrap() {
                println!("{} '{}' {} bytes, {} files", s.name, s.title, s.size, s.files);
            }
        }
        Some("dumpsave") => {
            let (fmt, s) = formats::load_file(&a[2]).unwrap();
            println!("{fmt:?} {} '{}'", s.name, s.title());
            for f in &s.files {
                println!("  {} {} {:08x}", f.name, f.data.len(), crc32fast::hash(&f.data));
            }
        }
        _ => eprintln!("usage: interop mkcard|mksave|readcard|dumpsave ..."),
    }
}
