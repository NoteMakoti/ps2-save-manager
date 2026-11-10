//! `ps2sm` — command line PS2 memory card / save manager.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use ps2mc::{formats, AnySave, MemoryCard, Platform, Ps1Card, Save, SaveFormat};
use ps2net::client::{MmceMode, MmceTarget};
use ps2net::{MemcardPro, Ps2Client};

#[derive(Parser)]
#[command(name = "ps2sm", version, about = "PS2 memory card and save manager")]
struct Cli {
    #[command(subcommand)]
    cmd: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Show card image information
    Info { card: PathBuf },
    /// List saves on a card image
    Saves { card: PathBuf },
    /// List a directory inside a card image
    Ls {
        card: PathBuf,
        #[arg(default_value = "/")]
        path: String,
    },
    /// Check a card image for filesystem problems
    Check {
        card: PathBuf,
        /// Free clusters that no file uses
        #[arg(long)]
        fix: bool,
    },
    /// Create a new, formatted card image
    Format {
        out: PathBuf,
        /// Size in MB (8, 16, 32, 64, 128)
        #[arg(long, default_value_t = 8)]
        size: usize,
        /// Omit per-page ECC (plain 8,388,608 byte image)
        #[arg(long)]
        no_ecc: bool,
    },
    /// Export saves from a card image to .psu/.max/.cbs/.sps files
    Export {
        card: PathBuf,
        /// Save directory names (omit with --all)
        saves: Vec<String>,
        #[arg(long)]
        all: bool,
        #[arg(short, long, default_value = "psu")]
        format: String,
        /// Output file (single save) or directory
        #[arg(short, long, default_value = ".")]
        out: PathBuf,
    },
    /// Import save files into a card image
    Import {
        card: PathBuf,
        files: Vec<PathBuf>,
        #[arg(long)]
        overwrite: bool,
    },
    /// Delete saves from a card image
    Delete { card: PathBuf, saves: Vec<String> },
    /// Rename a save directory on a card image
    Rename { card: PathBuf, from: String, to: String },
    /// Upload files into a save folder on a card image
    Add {
        card: PathBuf,
        /// Save directory name
        save: String,
        files: Vec<PathBuf>,
        /// Replace files that already exist in the save
        #[arg(long)]
        replace: bool,
    },
    /// Copy files out of a save folder on a card image (all files if none given)
    Extract {
        card: PathBuf,
        save: String,
        files: Vec<String>,
        #[arg(short, long, default_value = ".")]
        out: PathBuf,
    },
    /// Delete files inside a save folder: SAVE/FILE ...
    Rm { card: PathBuf, paths: Vec<String> },
    /// Show what is inside a save file
    Show { file: PathBuf },
    /// Convert between save formats, or between card layouts (.ps2/.mc2 with ECC, .bin without)
    Convert {
        input: PathBuf,
        output: PathBuf,
        /// For card images: force ECC on/off in the output
        #[arg(long)]
        ecc: Option<bool>,
    },
    /// Talk to the PS2SM homebrew app on a PS2
    Ps2 {
        /// PS2 address (ip or ip:port)
        host: String,
        /// Memory card slot (1 or 2)
        #[arg(short, long, default_value_t = 1)]
        slot: u8,
        #[command(subcommand)]
        action: Ps2Action,
    },
    /// Serve card images over the PS2SM protocol (acts like a PS2)
    Serve {
        card1: Option<PathBuf>,
        card2: Option<PathBuf>,
        #[arg(long, default_value = "0.0.0.0:6524")]
        bind: String,
    },
    /// MemCard PRO2 over Wi-Fi (FTP)
    Mcp {
        host: String,
        #[arg(short, long, default_value = "admin")]
        user: String,
        #[arg(short, long, default_value = "")]
        password: String,
        #[command(subcommand)]
        action: McpAction,
    },
}

#[derive(Subcommand)]
enum Ps2Action {
    /// Server and card status
    Status,
    /// List saves on the card
    Saves,
    /// Download saves
    Get {
        saves: Vec<String>,
        #[arg(short, long, default_value = "psu")]
        format: String,
        #[arg(short, long, default_value = ".")]
        out: PathBuf,
    },
    /// Upload save files
    Put {
        files: Vec<PathBuf>,
        #[arg(long)]
        overwrite: bool,
    },
    /// Delete saves on the PS2's card
    Delete { saves: Vec<String> },
    /// List the files inside a save folder
    Ls { save: String },
    /// Upload files into a save folder on the PS2's card
    Add {
        save: String,
        files: Vec<PathBuf>,
        #[arg(long)]
        replace: bool,
    },
    /// Copy files out of a save folder (all files if none given)
    Extract {
        save: String,
        files: Vec<String>,
        #[arg(short, long, default_value = ".")]
        out: PathBuf,
    },
    /// Delete files inside a save folder: SAVE/FILE ...
    Rm { paths: Vec<String> },
    /// Copy the whole card into a local image
    Dump {
        out: PathBuf,
        #[arg(long)]
        no_ecc: bool,
    },
    /// Copy every save from a local image to the PS2's card
    Restore {
        image: PathBuf,
        #[arg(long)]
        overwrite: bool,
    },
    /// MMCE (MemCard PRO2 / SD2PSX) card switching: status, next, prev, card N, channel N
    Mmce { args: Vec<String> },
}

#[derive(Subcommand)]
enum McpAction {
    /// List virtual cards on the device
    Cards,
    /// List a remote directory
    Ls {
        #[arg(default_value = "/")]
        path: String,
    },
    /// Download a remote file (e.g. a .mc2 card)
    Get { remote: String, local: PathBuf },
    /// Upload a local file (keeps <remote>.bak unless --no-backup)
    Put {
        local: PathBuf,
        remote: String,
        #[arg(long)]
        no_backup: bool,
    },
    /// Export a whole memory card as one virtual card:
    /// /PS2/<NAME>/<NAME>-<CHANNEL>.mc2 (or /PS1/…/.mcd for PS1 cards)
    UploadCard {
        /// Card image (.ps2/.mc2/.bin/.vmc or PS1 .mcd/.mcr/.gme); omit with --ps2
        image: Option<PathBuf>,
        /// Take the card from a PS2 running PS2SM instead
        #[arg(long)]
        ps2: Option<String>,
        /// PS2 slot for --ps2
        #[arg(long, default_value_t = 1)]
        slot: u8,
        /// Folder/card name on the SD card (default: image file name, or a Game ID)
        #[arg(long)]
        name: Option<String>,
        #[arg(long, default_value_t = 1)]
        channel: u32,
        /// Replace an existing card (previous one kept as .bak)
        #[arg(long)]
        replace: bool,
    },
    /// Give every game its own Game ID card: /PS2/<GAMEID>/<GAMEID>-1.mc2
    /// (PS1 saves: /PS1/<GAMEID>/<GAMEID>-1.mcd). Saves are merged into
    /// cards that already exist.
    PerGame {
        /// Card images and/or save files (.psu .max .cbs .sps .xps .mcs)
        sources: Vec<PathBuf>,
        /// Also take every save from a PS2 running PS2SM
        #[arg(long)]
        ps2: Option<String>,
        #[arg(long, default_value_t = 1)]
        slot: u8,
        /// Only these save directories (default: all)
        #[arg(long = "only", num_args = 1..)]
        only: Vec<String>,
        #[arg(long, default_value_t = 1)]
        channel: u32,
        /// Replace saves that already exist on a game's card
        #[arg(long)]
        overwrite: bool,
        /// Don't create name.txt with the game title
        #[arg(long)]
        no_names: bool,
        /// Show what would be written without uploading anything
        #[arg(long)]
        dry_run: bool,
    },
}

/// Every save in card images or save files, PS1 and PS2.
fn collect_saves(paths: &[PathBuf]) -> Result<Vec<AnySave>> {
    let mut out = Vec::new();
    for p in paths {
        let data = std::fs::read(p).with_context(|| format!("reading {}", p.display()))?;
        if let Ok(c) = MemoryCard::open(data.clone()) {
            for s in c.saves()? {
                out.push(AnySave::Ps2(c.export_save(&s.name)?));
            }
        } else if ps2mc::ps1::looks_like(&data) {
            let c = Ps1Card::open(data)?;
            for s in c.saves() {
                out.push(AnySave::Ps1(c.export_save(&s.name)?));
            }
        } else {
            out.push(ps2mc::anysave::load_any(&data).with_context(|| format!("{}: not a card image or save file", p.display()))?);
        }
    }
    Ok(out)
}

fn ps2_saves(host: &str, slot: u8) -> Result<Vec<AnySave>> {
    let mut c = Ps2Client::connect(host)?;
    let mut out = Vec::new();
    for s in c.list_saves(slot.saturating_sub(1))? {
        println!("reading {} from the PS2...", s.name);
        out.push(AnySave::Ps2(c.get_save(slot.saturating_sub(1), &s.name)?));
    }
    c.bye();
    Ok(out)
}

fn fmt_from(s: &str) -> Result<SaveFormat> {
    SaveFormat::from_extension(s).with_context(|| format!("unknown format '{s}' (psu, max, cbs, sps)"))
}

fn open_card(p: &Path) -> Result<MemoryCard> {
    MemoryCard::open_file(p).with_context(|| format!("opening {}", p.display()))
}

fn print_saves(card: &MemoryCard) -> Result<()> {
    let saves = card.saves()?;
    println!("{:<32} {:>8}  {:<19}  title", "directory", "size", "modified");
    for s in &saves {
        println!("{:<32} {:>7}K  {}  {}", s.name, s.size.div_ceil(1024), s.modified, s.title);
    }
    println!("{} save(s)", saves.len());
    Ok(())
}

fn write_save(save: &Save, fmt: SaveFormat, out: &Path, single: bool) -> Result<PathBuf> {
    let path = if single && out.extension().is_some() && !out.is_dir() {
        out.to_path_buf()
    } else {
        std::fs::create_dir_all(out)?;
        out.join(save.suggested_filename(fmt.extension()))
    };
    std::fs::write(&path, fmt.save(save)?)?;
    Ok(path)
}

/// Read local files for upload, checking names and clashes with `existing`.
fn files_to_upload(files: &[PathBuf], existing: &[String], replace: bool) -> Result<Vec<(String, Vec<u8>)>> {
    let mut out = Vec::new();
    for f in files {
        let name = f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if !ps2mc::dirent::valid_name(&name) {
            bail!("'{name}' is not a valid memory card file name (max 31 ASCII characters)");
        }
        if !replace && existing.contains(&name) {
            bail!("{name} already exists in the save (use --replace)");
        }
        out.push((name, std::fs::read(f).with_context(|| format!("reading {}", f.display()))?));
    }
    Ok(out)
}

fn extract_files(save: &Save, names: &[String], out: &Path) -> Result<()> {
    std::fs::create_dir_all(out)?;
    for n in names {
        if save.file(n).is_none() {
            bail!("{n} not found in {}", save.name);
        }
    }
    for f in save.files.iter().filter(|f| names.is_empty() || names.contains(&f.name)) {
        let p = out.join(&f.name);
        std::fs::write(&p, &f.data)?;
        println!("{} -> {}", f.name, p.display());
    }
    Ok(())
}

fn is_card_path(p: &Path) -> bool {
    matches!(
        p.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref(),
        Some("ps2" | "mc2" | "bin" | "vmc" | "mcd" | "mc")
    )
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Command::Info { card } => {
            let c = open_card(&card)?;
            let i = c.info()?;
            println!("image size : {} bytes", i.image_size);
            println!("ECC        : {}", if i.has_ecc { "yes" } else { "no" });
            println!("version    : {}", i.version);
            println!("page size  : {}  cluster size: {}", i.page_size, i.cluster_size);
            println!("capacity   : {} KB", i.total_bytes / 1024);
            println!("free       : {} KB", i.free_bytes / 1024);
            println!("saves      : {}", c.saves()?.len());
        }
        Command::Saves { card } => {
            let data = std::fs::read(&card)?;
            if ps2mc::ps1::looks_like(&data) && MemoryCard::open(data.clone()).is_err() {
                let c = Ps1Card::open(data)?;
                println!("{:<24} {:>6}  title", "name", "blocks");
                for s in c.saves() {
                    println!("{:<24} {:>6}  {}", s.name, s.blocks, s.title);
                }
                println!("{} save(s), {} free block(s)", c.saves().len(), c.free_blocks());
            } else {
                print_saves(&open_card(&card)?)?
            }
        }
        Command::Ls { card, path } => {
            let c = open_card(&card)?;
            for e in c.list(&path)? {
                println!(
                    "{:04x} {:>9} {} {}{}",
                    e.mode.0,
                    e.length,
                    e.modified,
                    e.name,
                    if e.mode.is_dir() { "/" } else { "" }
                );
            }
        }
        Command::Check { card, fix } => {
            let mut c = open_card(&card)?;
            let problems = c.check()?;
            if problems.is_empty() {
                println!("no problems found");
            }
            for p in &problems {
                println!("! {p}");
            }
            if fix {
                let n = c.reclaim_lost_clusters()?;
                if n > 0 {
                    c.save_to(&card)?;
                    println!("freed {n} lost cluster(s)");
                }
            }
        }
        Command::Format { out, size, no_ecc } => {
            if out.exists() {
                bail!("{} already exists", out.display());
            }
            let mut c = MemoryCard::format(size, !no_ecc)?;
            c.save_to(&out)?;
            println!("created {} ({} bytes)", out.display(), c.as_bytes().len());
        }
        Command::Export { card, saves, all, format, out } => {
            let c = open_card(&card)?;
            let fmt = fmt_from(&format)?;
            let names: Vec<String> = if all { c.saves()?.into_iter().map(|s| s.name).collect() } else { saves };
            if names.is_empty() {
                bail!("nothing to export (give save names or --all)");
            }
            let single = names.len() == 1;
            for n in names {
                let s = c.export_save(&n)?;
                let p = write_save(&s, fmt, &out, single)?;
                println!("{n} -> {}", p.display());
            }
        }
        Command::Import { card, files, overwrite } => {
            let mut c = open_card(&card)?;
            for f in files {
                let (fmt, s) = formats::load_file(&f).with_context(|| format!("reading {}", f.display()))?;
                c.import_save(&s, overwrite).with_context(|| format!("importing {}", s.name))?;
                println!("imported {} ({fmt:?}) {}", s.name, s.title());
            }
            c.save_to(&card)?;
        }
        Command::Delete { card, saves } => {
            let mut c = open_card(&card)?;
            for s in saves {
                c.remove_all(&s)?;
                println!("deleted {s}");
            }
            c.save_to(&card)?;
        }
        Command::Add { card, save, files, replace } => {
            let mut c = open_card(&card)?;
            let existing: Vec<String> = c.list(&save)?.into_iter().map(|e| e.name).collect();
            for (name, data) in files_to_upload(&files, &existing, replace)? {
                c.write_file(&format!("{save}/{name}"), &data)?;
                println!("added {save}/{name} ({} bytes)", data.len());
            }
            c.save_to(&card)?;
        }
        Command::Extract { card, save, files, out } => {
            let c = open_card(&card)?;
            extract_files(&c.export_save(&save)?, &files, &out)?;
        }
        Command::Rm { card, paths } => {
            let mut c = open_card(&card)?;
            for p in paths {
                if !p.trim_matches('/').contains('/') {
                    bail!("{p}: give SAVE/FILE (use `delete` to remove a whole save)");
                }
                c.remove(&p)?;
                println!("removed {p}");
            }
            c.save_to(&card)?;
        }
        Command::Rename { card, from, to } => {
            let mut c = open_card(&card)?;
            c.rename(&from, &to)?;
            c.save_to(&card)?;
        }
        Command::Show { file } => {
            let (fmt, s) = formats::load_file(&file)?;
            println!("format   : {}", fmt);
            println!("directory: {}", s.name);
            println!("title    : {}", s.title());
            if let Some(id) = s.game_id() {
                println!("game id  : {id} ({})", s.region());
            }
            println!("modified : {}", s.modified);
            println!("size     : {} KB on card", s.card_size_kb());
            for f in &s.files {
                println!("  {:<32} {:>8}  {}", f.name, f.data.len(), f.modified);
            }
        }
        Command::Convert { input, output, ecc } => {
            if is_card_path(&input) {
                let c = open_card(&input)?;
                let want = ecc.unwrap_or_else(|| {
                    !output.extension().map(|e| e.eq_ignore_ascii_case("bin")).unwrap_or(false)
                });
                std::fs::write(&output, c.to_bytes_with_ecc(want))?;
                println!("wrote {} ({})", output.display(), if want { "with ECC" } else { "no ECC" });
            } else {
                let (_, s) = formats::load_file(&input)?;
                let fmt = SaveFormat::from_path(&output).context("output extension must be .psu/.max/.cbs/.sps/.xps")?;
                std::fs::write(&output, fmt.save(&s)?)?;
                println!("wrote {}", output.display());
            }
        }
        Command::Ps2 { host, slot, action } => {
            if !(1..=2).contains(&slot) {
                bail!("slot must be 1 or 2");
            }
            let port = slot - 1;
            let mut c = Ps2Client::connect(&host).with_context(|| format!("connecting to {host}"))?;
            match action {
                Ps2Action::Status => {
                    println!("server  : {} {} (protocol {})", c.info.name, c.info.version, c.info.protocol);
                    println!("caps    : {:#x}", c.info.caps);
                    for p in 0..2u8 {
                        let i = c.card_info(p)?;
                        if i.present {
                            println!(
                                "slot {}  : type {} {} free {} KB",
                                p + 1,
                                i.card_type,
                                if i.formatted { "formatted" } else { "unformatted" },
                                i.free_kb
                            );
                        } else {
                            println!("slot {}  : empty", p + 1);
                        }
                    }
                }
                Ps2Action::Saves => {
                    for s in c.list_saves(port)? {
                        println!("{:<32} {:>7}K  {}  {}", s.name, s.size.div_ceil(1024), s.modified, s.title);
                    }
                }
                Ps2Action::Get { saves, format, out } => {
                    let fmt = fmt_from(&format)?;
                    let single = saves.len() == 1;
                    for n in saves {
                        let s = c.get_save(port, &n)?;
                        let p = write_save(&s, fmt, &out, single)?;
                        println!("{n} -> {}", p.display());
                    }
                }
                Ps2Action::Put { files, overwrite } => {
                    for f in files {
                        let (_, s) = formats::load_file(&f)?;
                        c.put_save(port, &s, overwrite)?;
                        println!("uploaded {}", s.name);
                    }
                }
                Ps2Action::Ls { save } => {
                    for e in c.list_dir(port, &save)? {
                        println!("{:04x} {:>9} {} {}", e.mode.0, e.size, e.modified, e.name);
                    }
                }
                Ps2Action::Add { save, files, replace } => {
                    let existing: Vec<String> = c.list_dir(port, &save)?.into_iter().map(|e| e.name).collect();
                    for (name, data) in files_to_upload(&files, &existing, replace)? {
                        c.write_file(port, &format!("{save}/{name}"), &data)?;
                        println!("added {save}/{name} ({} bytes)", data.len());
                    }
                }
                Ps2Action::Extract { save, files, out } => {
                    extract_files(&c.get_save(port, &save)?, &files, &out)?;
                }
                Ps2Action::Rm { paths } => {
                    for p in paths {
                        if !p.trim_matches('/').contains('/') {
                            bail!("{p}: give SAVE/FILE (use `delete` to remove a whole save)");
                        }
                        c.delete(port, &p)?;
                        println!("removed {p}");
                    }
                }
                Ps2Action::Delete { saves } => {
                    for s in saves {
                        c.delete_save(port, &s)?;
                        println!("deleted {s}");
                    }
                }
                Ps2Action::Dump { out, no_ecc } => {
                    let mut img = c.dump_to_image(port, !no_ecc, |i, n, name| {
                        if !name.is_empty() {
                            println!("[{}/{}] {}", i + 1, n, name);
                        }
                    })?;
                    img.save_to(&out)?;
                    println!("wrote {}", out.display());
                }
                Ps2Action::Restore { image, overwrite } => {
                    let img = open_card(&image)?;
                    for s in img.saves()? {
                        let save = img.export_save(&s.name)?;
                        match c.put_save(port, &save, overwrite) {
                            Ok(()) => println!("uploaded {}", s.name),
                            Err(e) => println!("skipped {}: {e}", s.name),
                        }
                    }
                }
                Ps2Action::Mmce { args } => {
                    let a: Vec<&str> = args.iter().map(String::as_str).collect();
                    match a.as_slice() {
                        [] | ["status"] => {}
                        ["next"] => {
                            c.mmce_set(port, MmceTarget::Card, MmceMode::Next)?;
                        }
                        ["prev"] => {
                            c.mmce_set(port, MmceTarget::Card, MmceMode::Prev)?;
                        }
                        ["card", n] => {
                            c.mmce_set(port, MmceTarget::Card, MmceMode::Set(n.parse()?))?;
                        }
                        ["channel", "next"] => {
                            c.mmce_set(port, MmceTarget::Channel, MmceMode::Next)?;
                        }
                        ["channel", "prev"] => {
                            c.mmce_set(port, MmceTarget::Channel, MmceMode::Prev)?;
                        }
                        ["channel", n] => {
                            c.mmce_set(port, MmceTarget::Channel, MmceMode::Set(n.parse()?))?;
                        }
                        _ => bail!("usage: mmce [status|next|prev|card N|channel N]"),
                    }
                    if !a.is_empty() && a != ["status"] {
                        std::thread::sleep(std::time::Duration::from_millis(1200));
                    }
                    let s = c.mmce_status(port)?;
                    println!("card {} channel {} game id '{}'", s.card, s.channel, s.game_id);
                }
            }
            c.bye();
        }
        Command::Serve { card1, card2, bind } => {
            let load = |p: Option<PathBuf>| -> Result<Option<ps2net::server::Slot>> {
                Ok(match p {
                    Some(p) => Some(ps2net::server::Slot { card: open_card(&p)?, path: Some(p) }),
                    None => None,
                })
            };
            let slots = ps2net::server::slots(load(card1)?, load(card2)?);
            ps2net::server::serve(&bind, slots, |m| println!("{m}"))?;
        }
        Command::Mcp { host, user, password, action } => {
            let mut m = MemcardPro::connect(&host, &user, &password)?;
            match action {
                McpAction::Cards => {
                    for c in m.find_cards()? {
                        println!("{} {:<60} {:>10}  channel {:?}", c.platform.folder(), c.path, c.size, c.channel());
                    }
                }
                McpAction::UploadCard { image, ps2, slot, name, channel, replace } => {
                    let (platform, bytes, default_name) = match (&image, &ps2) {
                        (Some(p), None) => {
                            let data = std::fs::read(p)?;
                            let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                            // strip a "-1" style channel suffix from the file name
                            let stem = match stem.rsplit_once('-') {
                                Some((a, b)) if b.len() == 1 && b.chars().all(|c| c.is_ascii_digit()) => a.to_string(),
                                _ => stem,
                            };
                            if let Ok(c) = MemoryCard::open(data.clone()) {
                                (Platform::Ps2, c.to_bytes_with_ecc(true), stem)
                            } else if ps2mc::ps1::looks_like(&data) {
                                (Platform::Ps1, Ps1Card::open(data)?.as_bytes().to_vec(), stem)
                            } else {
                                bail!("{} is not a memory card image", p.display());
                            }
                        }
                        (None, Some(host)) => {
                            let mut c = Ps2Client::connect(host)?;
                            let img = c.dump_to_image(slot.saturating_sub(1), true, |i, n, s| {
                                if !s.is_empty() {
                                    println!("[{}/{}] {s}", i + 1, n)
                                }
                            })?;
                            c.bye();
                            (Platform::Ps2, img.into_bytes(), format!("PS2Slot{slot}"))
                        }
                        _ => bail!("give either an image file or --ps2 HOST"),
                    };
                    let folder = name.unwrap_or(default_name);
                    let path = ps2net::pro2::export_whole_card(&mut m, platform, &folder, channel, &bytes, replace)?;
                    println!("uploaded {} bytes to {path}", bytes.len());
                }
                McpAction::PerGame { sources, ps2, slot, only, channel, overwrite, no_names, dry_run } => {
                    let mut saves = collect_saves(&sources)?;
                    if let Some(h) = &ps2 {
                        saves.extend(ps2_saves(h, slot)?);
                    }
                    if !only.is_empty() {
                        saves.retain(|s| only.iter().any(|o| o == s.name()));
                    }
                    let groups = ps2mc::anysave::group_by_game(saves);
                    for ((p, id), list) in &groups.games {
                        let names: Vec<&str> = list.iter().map(|s| s.name()).collect();
                        println!("{}  <-  {}", ps2net::pro2::card_path(*p, id, channel), names.join(", "));
                    }
                    for s in &groups.unmatched {
                        println!("(skipped, no game ID) {}", s.name());
                    }
                    if !dry_run {
                        let rep = ps2net::pro2::export_per_game(&mut m, &groups, channel, overwrite, !no_names, |i, n, id| {
                            if !id.is_empty() {
                                println!("[{}/{}] {id}", i + 1, n)
                            }
                        })?;
                        for (id, e) in &rep.failed {
                            println!("! {id}: {e}");
                        }
                        for k in &rep.kept {
                            println!("= {k} already on its card (use --overwrite to replace)");
                        }
                        println!("{}", rep.summary());
                    }
                }
                McpAction::Ls { path } => {
                    for e in m.list(&path)? {
                        println!("{}{:<40} {:>10}", if e.is_dir { "d " } else { "  " }, e.name, e.size);
                    }
                }
                McpAction::Get { remote, local } => {
                    let d = m.download(&remote)?;
                    std::fs::write(&local, &d)?;
                    println!("{} bytes -> {}", d.len(), local.display());
                }
                McpAction::Put { local, remote, no_backup } => {
                    let d = std::fs::read(&local)?;
                    if remote.to_ascii_lowercase().ends_with(".mc2") {
                        MemoryCard::open(d.clone()).context("refusing to upload: not a valid card image")?;
                    }
                    m.upload(&remote, &d, !no_backup)?;
                    println!("uploaded {} bytes", d.len());
                }
            }
            m.quit();
        }
    }
    Ok(())
}
