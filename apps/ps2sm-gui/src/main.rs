//! PS2 Save Manager — desktop GUI.
//!
//! Two panes side by side, each showing one "source": a card image, a
//! PS2 running the PS2SM homebrew app, a MemCard PRO2 virtual card, or a
//! folder of save files (which can live on the same SMB share the PS2
//! app backs up to). Saves are copied between panes with the arrow
//! buttons; everything slow runs on worker threads.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod backend;
mod settings;

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};

use backend::{Backend, McpOrigin, SaveRow, Status};
use eframe::egui;
use egui_extras::{Column, TableBuilder};
use ps2mc::anysave::{AnySave, Platform};
use ps2mc::{formats, MemoryCard, Ps1Card, Save, SaveFormat};
use ps2net::client::{MmceMode, MmceTarget};
use ps2net::memcardpro::VirtualCard;
use ps2net::{MemcardPro, Ps2Client};
use settings::Settings;

type Shared = Arc<Mutex<Backend>>;

struct Source {
    id: u64,
    label: String,
    kind: &'static str,
    backend: Shared,
    rows: Vec<SaveRow>,
    selected: BTreeSet<String>,
    status: Status,
    error: Option<String>,
    busy: bool,
    mmce: String,
}

enum Msg {
    Log(String),
    Error(String),
    Listed { id: u64, result: Result<(Vec<SaveRow>, Status), String> },
    Refresh(Vec<u64>),
    AddSource { label: String, backend: Backend },
    McpCards(Result<Vec<VirtualCard>, String>),
    Explore { source: Option<u64>, key: String, save: Box<Save> },
    ExplorerIdle,
    Mmce { id: u64, text: String },
    Relabel { id: u64, label: String },
    McpIdle,
    Idle(u64),
}

#[derive(Default)]
struct Pane {
    source: Option<u64>,
    filter: String,
}

struct RenameDialog {
    source: u64,
    key: String,
    new_name: String,
}

/// The file explorer for one save folder.
struct Explorer {
    /// `None` when the save came from a loose file (read-only view).
    source: Option<u64>,
    key: String,
    save: Save,
    selected: BTreeSet<String>,
    replace: bool,
    busy: bool,
    rename: Option<(String, String)>,
    confirm_delete: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Pro2Mode {
    WholeCard,
    PerGame,
}

/// "Export to MemCard PRO2" dialog state.
struct Pro2Dialog {
    source: u64,
    mode: Pro2Mode,
    folder: String,
    channel: u32,
    replace: bool,
    only_selected: bool,
    overwrite: bool,
    names: bool,
}

/// Make a source label usable as a folder name on the SD card.
fn folder_name_from(label: &str) -> String {
    let base = label
        .trim_start_matches("PRO2 ")
        .trim_start_matches("PS1: ")
        .rsplit('/')
        .next()
        .unwrap_or(label);
    let stem = match base.rsplit_once('.') {
        Some((a, _)) if !a.is_empty() => a,
        _ => base,
    };
    // drop a "-1" channel suffix
    let stem = match stem.rsplit_once('-') {
        Some((a, b)) if b.len() == 1 && b.chars().all(|c| c.is_ascii_digit()) => a,
        _ => stem,
    };
    let s: String = stem
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || "-_.()".contains(c) { c } else { '_' })
        .collect::<String>()
        .trim_matches('_')
        .to_string();
    if s.is_empty() { "MemoryCard".into() } else { s }
}

struct App {
    settings: Settings,
    sources: Vec<Source>,
    next_id: u64,
    panes: [Pane; 2],
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    log: Vec<(bool, String)>,
    overwrite: bool,
    export_format: SaveFormat,
    mcp_password: String,
    mcp_cards: Option<Vec<VirtualCard>>,
    mcp_busy: bool,
    ps2_busy: bool,
    explorer: Option<Explorer>,
    rename: Option<RenameDialog>,
    confirm_delete: Option<(u64, Vec<String>)>,
    pro2: Option<Pro2Dialog>,
    ctx: egui::Context,
}

fn install_cjk_font(ctx: &egui::Context) {
    // Save titles are Shift-JIS; the default egui fonts have no kana/kanji.
    let candidates = [
        "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/truetype/droid/DroidSansFallbackFull.ttf",
        "/usr/share/fonts/TTF/NotoSansCJK-Regular.ttc",
        "C:\\Windows\\Fonts\\msgothic.ttc",
        "C:\\Windows\\Fonts\\YuGothM.ttc",
        "/System/Library/Fonts/ヒラギノ角ゴシック W3.ttc",
        "/System/Library/Fonts/Hiragino Sans GB.ttc",
    ];
    for p in candidates {
        if let Ok(bytes) = std::fs::read(p) {
            let mut fonts = egui::FontDefinitions::default();
            fonts.font_data.insert("cjk".into(), Arc::new(egui::FontData::from_owned(bytes)));
            for fam in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
                fonts.families.entry(fam).or_default().push("cjk".into());
            }
            ctx.set_fonts(fonts);
            return;
        }
    }
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> App {
        install_cjk_font(&cc.egui_ctx);
        let (tx, rx) = channel();
        let settings = Settings::load();
        let export_format = SaveFormat::from_extension(&settings.export_format).unwrap_or(SaveFormat::Psu);
        let mut app = App {
            settings,
            sources: Vec::new(),
            next_id: 1,
            panes: Default::default(),
            tx,
            rx,
            log: Vec::new(),
            overwrite: false,
            export_format,
            mcp_password: String::new(),
            mcp_cards: None,
            mcp_busy: false,
            ps2_busy: false,
            explorer: None,
            rename: None,
            confirm_delete: None,
            pro2: None,
            ctx: cc.egui_ctx.clone(),
        };
        // usage: ps2sm-gui [--ps2 HOST] [card-or-folder ...]
        let mut args = std::env::args().skip(1);
        let mut connect = false;
        while let Some(a) = args.next() {
            if a == "--ps2" {
                if let Some(h) = args.next() {
                    app.settings.ps2_host = h;
                    connect = true;
                }
            } else if PathBuf::from(&a).is_dir() {
                app.add_library(PathBuf::from(a));
            } else {
                app.open_path(PathBuf::from(a));
            }
        }
        if connect {
            app.connect_ps2();
        }
        if let Some(dir) = app.settings.library_dir.clone() {
            if dir.is_dir() {
                app.add_library(dir);
            }
        }
        app
    }

    // ---------------------------------------------------------------
    // Sources
    // ---------------------------------------------------------------

    fn add_source(&mut self, label: String, backend: Backend) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let kind = backend.kind();
        self.sources.push(Source {
            id,
            label,
            kind,
            backend: Arc::new(Mutex::new(backend)),
            rows: vec![],
            selected: BTreeSet::new(),
            status: Status::default(),
            error: None,
            busy: false,
            mmce: String::new(),
        });
        // show it in an empty pane (otherwise pick it from a pane's list)
        if let Some(p) = self.panes.iter_mut().find(|p| p.source.is_none()) {
            p.source = Some(id);
        }
        self.refresh(id);
        id
    }

    fn source(&self, id: u64) -> Option<&Source> {
        self.sources.iter().find(|s| s.id == id)
    }

    fn source_mut(&mut self, id: u64) -> Option<&mut Source> {
        self.sources.iter_mut().find(|s| s.id == id)
    }

    fn refresh(&mut self, id: u64) {
        let Some(src) = self.source_mut(id) else { return };
        src.busy = true;
        let be = src.backend.clone();
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let result = be.lock().unwrap().list().map_err(|e| format!("{e:#}"));
            let _ = tx.send(Msg::Listed { id, result });
            ctx.request_repaint();
        });
    }

    /// Run `f` on a worker thread, then refresh the listed sources.
    fn job(&mut self, busy: &[u64], f: impl FnOnce(&Sender<Msg>) -> anyhow::Result<()> + Send + 'static) {
        for id in busy {
            if let Some(s) = self.source_mut(*id) {
                s.busy = true;
            }
        }
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        let ids = busy.to_vec();
        std::thread::spawn(move || {
            if let Err(e) = f(&tx) {
                let _ = tx.send(Msg::Error(format!("{e:#}")));
            }
            let _ = tx.send(Msg::Refresh(ids));
            ctx.request_repaint();
        });
    }

    fn open_path(&mut self, p: PathBuf) {
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        match std::fs::read(&p) {
            Ok(data) => {
                if let Ok(card) = MemoryCard::open(data.clone()) {
                    self.add_source(name, Backend::Card { card, path: Some(p), mcp: None });
                } else if ps2mc::ps1::looks_like(&data) {
                    match Ps1Card::open(data) {
                        Ok(card) => {
                            self.add_source(format!("PS1: {name}"), Backend::Ps1 { card, path: Some(p), mcp: None });
                        }
                        Err(e) => self.err(format!("{name}: {e}")),
                    }
                } else if ps2mc::ps1::is_mcs(&data) {
                    self.log(format!("{name} is a PS1 save (.mcs); open a save folder or PS1 card to copy it"));
                } else if let Ok((fmt, save)) = formats::load_any(&data) {
                    self.log(format!("{} is a {} save; showing its files (read-only)", name, fmt.extension()));
                    self.set_explorer(None, p.to_string_lossy().into_owned(), save);
                } else {
                    self.err(format!("{name}: not a memory card image or save file"));
                }
            }
            Err(e) => self.err(format!("{name}: {e}")),
        }
    }

    fn add_library(&mut self, dir: PathBuf) {
        let fmt = self.export_format;
        let label = format!("Folder: {}", dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| dir.display().to_string()));
        self.add_source(label, Backend::Library { dir, format: fmt });
    }

    fn log(&mut self, m: String) {
        self.log.push((false, m));
    }

    fn err(&mut self, m: String) {
        self.log.push((true, m));
    }

    fn handle_messages(&mut self) {
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Msg::Log(s) => self.log(s),
                Msg::Error(s) => self.err(s),
                Msg::Listed { id, result } => {
                    if let Some(src) = self.source_mut(id) {
                        src.busy = false;
                        match result {
                            Ok((rows, status)) => {
                                src.selected.retain(|k| rows.iter().any(|r| &r.key == k));
                                src.rows = rows;
                                src.status = status;
                                src.error = None;
                            }
                            Err(e) => {
                                src.error = Some(e);
                                src.rows.clear();
                            }
                        }
                    }
                }
                Msg::Refresh(ids) => {
                    for id in ids {
                        self.refresh(id);
                    }
                }
                Msg::AddSource { label, backend } => {
                    self.add_source(label, backend);
                }
                Msg::McpCards(r) => {
                    self.mcp_busy = false;
                    match r {
                        Ok(c) => {
                            self.log(format!("MemCard PRO2: found {} virtual card(s)", c.len()));
                            self.mcp_cards = Some(c);
                        }
                        Err(e) => self.err(format!("MemCard PRO2: {e}")),
                    }
                }
                Msg::Explore { source, key, save } => self.set_explorer(source, key, *save),
                Msg::ExplorerIdle => {
                    if let Some(e) = &mut self.explorer {
                        e.busy = false;
                    }
                }
                Msg::Mmce { id, text } => {
                    if let Some(s) = self.source_mut(id) {
                        s.mmce = text;
                    }
                }
                Msg::Relabel { id, label } => {
                    if let Some(s) = self.source_mut(id) {
                        s.label = label;
                    }
                }
                Msg::McpIdle => self.mcp_busy = false,
                Msg::Idle(id) => {
                    if let Some(s) = self.source_mut(id) {
                        s.busy = false;
                    }
                    self.ps2_busy = false;
                }
            }
        }
    }

    // ---------------------------------------------------------------
    // Actions
    // ---------------------------------------------------------------

    fn copy(&mut self, from: u64, to: u64) {
        let (Some(src), Some(dst)) = (self.source(from), self.source(to)) else { return };
        let keys: Vec<String> = src.selected.iter().cloned().collect();
        if keys.is_empty() {
            self.err("select one or more saves to copy".into());
            return;
        }
        let (sb, db) = (src.backend.clone(), dst.backend.clone());
        let dst_label = dst.label.clone();
        let overwrite = self.overwrite;
        self.job(&[from, to], move |tx| {
            for k in keys {
                let save = sb.lock().unwrap().get_any(&k)?;
                match db.lock().unwrap().put_any(&save, overwrite) {
                    Ok(()) => {
                        let _ = tx.send(Msg::Log(format!("copied {} → {}", save.name(), dst_label)));
                    }
                    Err(e) => {
                        let _ = tx.send(Msg::Error(format!("{}: {e:#}", save.name())));
                    }
                }
            }
            Ok(())
        });
    }

    fn export_selected(&mut self, id: u64) {
        let Some(src) = self.source(id) else { return };
        let keys: Vec<String> = src.selected.iter().cloned().collect();
        if keys.is_empty() {
            self.err("select one or more saves to export".into());
            return;
        }
        let Some(dir) = rfd::FileDialog::new().set_title("Export saves to folder").pick_folder() else { return };
        let fmt = self.export_format;
        let be = src.backend.clone();
        self.job(&[id], move |tx| {
            for k in keys {
                let s = be.lock().unwrap().get_any(&k)?;
                let (name, bytes) = ps2mc::anysave::export_bytes(&s, fmt)?;
                let p = dir.join(name);
                std::fs::write(&p, bytes)?;
                let _ = tx.send(Msg::Log(format!("exported {}", p.display())));
            }
            Ok(())
        });
    }

    fn import_files(&mut self, id: u64) {
        let Some(src) = self.source(id) else { return };
        let Some(files) = rfd::FileDialog::new()
            .set_title("Import save files")
            .add_filter("Save files", &["psu", "max", "cbs", "sps", "xps", "mcs"])
            .pick_files()
        else {
            return;
        };
        let be = src.backend.clone();
        let overwrite = self.overwrite;
        self.job(&[id], move |tx| {
            for f in files {
                match ps2mc::anysave::load_file(&f) {
                    Ok(s) => match be.lock().unwrap().put_any(&s, overwrite) {
                        Ok(()) => {
                            let _ = tx.send(Msg::Log(format!("imported {}", s.name())));
                        }
                        Err(e) => {
                            let _ = tx.send(Msg::Error(format!("{}: {e:#}", s.name())));
                        }
                    },
                    Err(e) => {
                        let _ = tx.send(Msg::Error(format!("{}: {e}", f.display())));
                    }
                }
            }
            Ok(())
        });
    }

    fn delete(&mut self, id: u64, keys: Vec<String>) {
        let Some(src) = self.source(id) else { return };
        let be = src.backend.clone();
        self.job(&[id], move |tx| {
            for k in keys {
                be.lock().unwrap().delete(&k)?;
                let _ = tx.send(Msg::Log(format!("deleted {k}")));
            }
            Ok(())
        });
    }

    /// Open the file explorer for save `key` of source `id`.
    fn show_details(&mut self, id: u64, key: String) {
        if self.source(id).is_none() {
            return;
        }
        self.explorer_job(id, key, |_| Ok(None));
    }

    /// Show (or refresh) the explorer, keeping selection and options when
    /// it is the same save.
    fn set_explorer(&mut self, source: Option<u64>, key: String, save: Save) {
        match &mut self.explorer {
            Some(e) if e.source == source && e.key == key => {
                e.selected.retain(|n| save.files.iter().any(|f| &f.name == n));
                e.save = save;
                e.busy = false;
            }
            _ => {
                self.explorer = Some(Explorer {
                    source,
                    key,
                    save,
                    selected: BTreeSet::new(),
                    replace: false,
                    busy: false,
                    rename: None,
                    confirm_delete: false,
                });
            }
        }
    }

    /// Run `op` on the explorer's backend in a worker thread, then reload
    /// the save into the explorer and refresh the pane listing.
    fn explorer_job(
        &mut self,
        id: u64,
        key: String,
        op: impl FnOnce(&mut Backend) -> anyhow::Result<Option<String>> + Send + 'static,
    ) {
        let Some(src) = self.source(id) else { return };
        let be = src.backend.clone();
        if let Some(e) = &mut self.explorer {
            e.busy = true;
        }
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let mut b = be.lock().unwrap();
            let changed = match op(&mut b) {
                Ok(Some(msg)) => {
                    let _ = tx.send(Msg::Log(msg));
                    true
                }
                Ok(None) => false,
                Err(e) => {
                    let _ = tx.send(Msg::Error(format!("{e:#}")));
                    true
                }
            };
            match b.get(&key) {
                Ok(s) => {
                    let _ = tx.send(Msg::Explore { source: Some(id), key, save: Box::new(s) });
                }
                Err(e) => {
                    let _ = tx.send(Msg::Error(format!("{e:#}")));
                    let _ = tx.send(Msg::ExplorerIdle);
                }
            }
            drop(b);
            if changed {
                let _ = tx.send(Msg::Refresh(vec![id]));
            }
            ctx.request_repaint();
        });
    }

    /// Upload local files into the explorer's save.
    fn upload_into_save(&mut self, paths: Vec<PathBuf>) {
        let Some(e) = &self.explorer else { return };
        let Some(id) = e.source else {
            self.err("this save was opened from a file and is read-only; import it into a card first".into());
            return;
        };
        let (key, replace) = (e.key.clone(), e.replace);
        self.explorer_job(id, key.clone(), move |b| {
            let mut files = Vec::new();
            for p in &paths {
                if p.is_dir() {
                    anyhow::bail!("{} is a folder; save folders can only hold files", p.display());
                }
                let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                files.push((name, std::fs::read(p)?));
            }
            b.put_files(&key, &files, replace)?;
            let names: Vec<&str> = files.iter().map(|(n, _)| n.as_str()).collect();
            Ok(Some(format!("uploaded {} into {key}", names.join(", "))))
        });
    }

    fn save_card(&mut self, id: u64, save_as: bool) {
        let Some(src) = self.source(id) else { return };
        let be = src.backend.clone();
        let target = if save_as {
            match rfd::FileDialog::new()
                .set_title("Save card image")
                .add_filter("Card with ECC (.ps2/.mc2)", &["ps2", "mc2"])
                .add_filter("Card without ECC (.bin)", &["bin"])
                .set_file_name("memcard.ps2")
                .save_file()
            {
                Some(p) => Some(p),
                None => return,
            }
        } else {
            None
        };
        self.job(&[id], move |tx| {
            let p = be.lock().unwrap().save_card(target.as_deref())?;
            let _ = tx.send(Msg::Log(format!("saved {}", p.display())));
            if let Some(n) = p.file_name() {
                let _ = tx.send(Msg::Relabel { id, label: n.to_string_lossy().into_owned() });
            }
            Ok(())
        });
    }

    fn connect_ps2(&mut self) {
        let host = self.settings.ps2_host.trim().to_string();
        if host.is_empty() {
            return;
        }
        self.settings.save();
        self.ps2_busy = true;
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            match Ps2Client::connect(&host) {
                Ok(c) => {
                    let _ = tx.send(Msg::Log(format!("connected to {} ({} {})", host, c.info.name, c.info.version)));
                    let client = Arc::new(Mutex::new(c));
                    for port in 0..2u8 {
                        let _ = tx.send(Msg::AddSource {
                            label: format!("🎮 {host} slot {}", port + 1),
                            backend: Backend::Ps2 { client: client.clone(), port, host: host.clone() },
                        });
                    }
                }
                Err(e) => {
                    let _ = tx.send(Msg::Error(format!("PS2 {host}: {e}")));
                }
            }
            let _ = tx.send(Msg::Idle(0));
            ctx.request_repaint();
        });
    }

    fn mcp_scan(&mut self) {
        self.settings.save();
        self.mcp_busy = true;
        let (h, u, p) = (self.settings.mcp_host.clone(), self.settings.mcp_user.clone(), self.mcp_password.clone());
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let r = MemcardPro::connect(&h, &u, &p).and_then(|mut m| {
                let c = m.find_cards();
                m.quit();
                c
            });
            let _ = tx.send(Msg::McpCards(r.map_err(|e| e.to_string())));
            ctx.request_repaint();
        });
    }

    fn mcp_open(&mut self, card: VirtualCard) {
        self.mcp_busy = true;
        let origin = McpOrigin {
            host: self.settings.mcp_host.clone(),
            user: self.settings.mcp_user.clone(),
            password: self.mcp_password.clone(),
            remote_path: card.path.clone(),
        };
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let platform = card.platform;
            let r = (|| -> anyhow::Result<Backend> {
                let mut m = MemcardPro::connect(&origin.host, &origin.user, &origin.password)?;
                let data = m.download(&origin.remote_path)?;
                m.quit();
                Ok(match platform {
                    Platform::Ps2 => Backend::Card { card: MemoryCard::open(data)?, path: None, mcp: Some(origin) },
                    Platform::Ps1 => Backend::Ps1 { card: Ps1Card::open(data)?, path: None, mcp: Some(origin) },
                })
            })();
            match r {
                Ok(backend) => {
                    let _ = tx.send(Msg::AddSource { label: format!("PRO2 {}/{}", card.group, card.file_name), backend });
                }
                Err(e) => {
                    let _ = tx.send(Msg::Error(format!("{}: {e:#}", card.path)));
                }
            }
            let _ = tx.send(Msg::McpIdle);
            ctx.request_repaint();
        });
    }

    fn mcp_upload(&mut self, id: u64) {
        let Some(src) = self.source(id) else { return };
        let be = src.backend.clone();
        self.job(&[id], move |tx| {
            let (data, origin) = {
                let b = be.lock().unwrap();
                match &*b {
                    Backend::Card { card, mcp: Some(o), .. } => (card.as_bytes().to_vec(), o.clone()),
                    Backend::Ps1 { card, mcp: Some(o), .. } => (card.as_bytes().to_vec(), o.clone()),
                    _ => anyhow::bail!("not a MemCard PRO2 card"),
                }
            };
            let mut m = MemcardPro::connect(&origin.host, &origin.user, &origin.password)?;
            m.upload(&origin.remote_path, &data, true)?;
            m.quit();
            match &mut *be.lock().unwrap() {
                Backend::Card { card, .. } => card.mark_clean(),
                Backend::Ps1 { card, .. } => card.mark_clean(),
                _ => {}
            }
            let _ = tx.send(Msg::Log(format!("uploaded {} (previous copy kept as .bak)", origin.remote_path)));
            Ok(())
        });
    }

    fn mmce(&mut self, id: u64, target: MmceTarget, mode: MmceMode) {
        let Some(src) = self.source(id) else { return };
        let be = src.backend.clone();
        self.job(&[id], move |tx| {
            let t = be.lock().unwrap().mmce(target, mode)?;
            let _ = tx.send(Msg::Mmce { id, text: t });
            Ok(())
        });
    }

    fn close_source(&mut self, id: u64) {
        self.sources.retain(|s| s.id != id);
        for p in &mut self.panes {
            if p.source == Some(id) {
                p.source = None;
            }
        }
    }

    // ---------------------------------------------------------------
    // UI
    // ---------------------------------------------------------------

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        ui.heading("Sources");
        ui.add_space(4.0);

        ui.collapsing("Card images", |ui| {
            ui.horizontal(|ui| {
                if ui.button("Open…").clicked() {
                    if let Some(files) = rfd::FileDialog::new()
                        .add_filter("Memory card images", &["ps2", "mc2", "bin", "vmc", "mcd", "mcr", "mc", "gme", "vgs", "mem"])
                        .add_filter("All files", &["*"])
                        .pick_files()
                    {
                        for f in files {
                            self.open_path(f);
                        }
                    }
                }
                if ui.button("New PS2 card").on_hover_text("Blank 8 MB card with ECC").clicked() {
                    match MemoryCard::format(8, true) {
                        Ok(c) => {
                            self.add_source("new card".into(), Backend::Card { card: c, path: None, mcp: None });
                        }
                        Err(e) => self.err(e.to_string()),
                    }
                }
                if ui.button("New PS1 card").on_hover_text("Blank 128 KB PS1 card (.mcd)").clicked() {
                    self.add_source("new PS1 card".into(), Backend::Ps1 { card: Ps1Card::format(), path: None, mcp: None });
                }
            });
        });

        ui.collapsing("PS2 over network", |ui| {
            ui.label("Run PS2SM on the console, then enter its IP.");
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.settings.ps2_host).hint_text("192.168.1.50").desired_width(130.0));
                let b = ui.add_enabled(!self.ps2_busy, egui::Button::new("Connect"));
                if b.clicked() {
                    self.connect_ps2();
                }
                if self.ps2_busy {
                    ui.spinner();
                }
            });
        });

        ui.collapsing("MemCard PRO2 (Wi-Fi)", |ui| {
            egui::Grid::new("mcp").num_columns(2).show(ui, |ui| {
                ui.label("Host");
                ui.add(egui::TextEdit::singleline(&mut self.settings.mcp_host).hint_text("memcardprogc or IP").desired_width(140.0));
                ui.end_row();
                ui.label("User");
                ui.add(egui::TextEdit::singleline(&mut self.settings.mcp_user).desired_width(140.0));
                ui.end_row();
                ui.label("Password");
                ui.add(egui::TextEdit::singleline(&mut self.mcp_password).password(true).desired_width(140.0));
                ui.end_row();
            });
            ui.horizontal(|ui| {
                if ui.add_enabled(!self.mcp_busy, egui::Button::new("Find cards")).clicked() {
                    self.mcp_scan();
                }
                if self.mcp_busy {
                    ui.spinner();
                }
            });
            ui.small("Enable the FTP server in the PRO2 WebUI first.");
            let mut open = None;
            if let Some(cards) = &self.mcp_cards {
                egui::ScrollArea::vertical().max_height(220.0).id_salt("mcpcards").show(ui, |ui| {
                    for c in cards {
                        let label = match c.channel() {
                            Some(ch) => format!("{} {} · ch {}", c.platform.folder(), c.group, ch),
                            None => format!("{} {} · {}", c.platform.folder(), c.group, c.file_name),
                        };
                        if ui.add_enabled(!self.mcp_busy, egui::Button::new(label).truncate()).on_hover_text(&c.path).clicked() {
                            open = Some(c.clone());
                        }
                    }
                });
            }
            if let Some(c) = open {
                self.mcp_open(c);
            }
        });

        ui.collapsing("Save folder / SMB share", |ui| {
            ui.label("A folder of .psu/.max/.cbs/.sps files. Point it at the share the PS2 app backs up to.");
            if ui.button("Choose folder…").clicked() {
                if let Some(d) = rfd::FileDialog::new().pick_folder() {
                    self.settings.library_dir = Some(d.clone());
                    self.settings.save();
                    self.add_library(d);
                }
            }
        });

        ui.separator();
        ui.label("Open:");
        let mut close = None;
        let mut save = None;
        let mut upload = None;
        for s in &self.sources {
            ui.horizontal(|ui| {
                if ui.small_button("✖").on_hover_text("Close").clicked() {
                    close = Some(s.id);
                }
                let dirty = if s.status.dirty { " •" } else { "" };
                ui.label(format!("{}{}", s.label, dirty)).on_hover_text(s.kind);
            });
            if s.status.dirty {
                ui.horizontal(|ui| {
                    ui.add_space(22.0);
                    let b = s.backend.lock().unwrap();
                    let (has_path, is_mcp) = match &*b {
                        Backend::Card { path, mcp, .. } => (path.is_some(), mcp.is_some()),
                        Backend::Ps1 { path, mcp, .. } => (path.is_some(), mcp.is_some()),
                        _ => (false, false),
                    };
                    drop(b);
                    if is_mcp {
                        if ui.small_button("Upload to PRO2").clicked() {
                            upload = Some(s.id);
                        }
                    }
                    if has_path {
                        if ui.small_button("Save").clicked() {
                            save = Some((s.id, false));
                        }
                    }
                    if ui.small_button("Save as…").clicked() {
                        save = Some((s.id, true));
                    }
                });
            }
        }
        if let Some(id) = close {
            let dirty = self.source(id).map(|s| s.status.dirty).unwrap_or(false);
            if dirty {
                self.err("unsaved changes discarded".into());
            }
            self.close_source(id);
        }
        if let Some((id, as_)) = save {
            self.save_card(id, as_);
        }
        if let Some(id) = upload {
            self.mcp_upload(id);
        }
    }

    fn pane(&mut self, ui: &mut egui::Ui, idx: usize) {
        let other = self.panes[1 - idx].source;
        let current = self.panes[idx].source;

        // source picker
        ui.horizontal(|ui| {
            let text = current.and_then(|id| self.source(id)).map(|s| s.label.clone()).unwrap_or_else(|| "— choose a source —".into());
            egui::ComboBox::from_id_salt(("pane", idx)).selected_text(text).width(260.0).show_ui(ui, |ui| {
                for s in &self.sources {
                    ui.selectable_value(&mut self.panes[idx].source, Some(s.id), format!("{}  ({})", s.label, s.kind));
                }
            });
            if let Some(id) = current {
                if ui.button("⟳").on_hover_text("Refresh").clicked() {
                    self.refresh(id);
                }
            }
        });

        let Some(id) = self.panes[idx].source else {
            ui.add_space(40.0);
            ui.vertical_centered(|ui| ui.label("Open a card image, connect to a PS2 or a MemCard PRO2,\nor choose a save folder from the left."));
            return;
        };
        let Some(src) = self.source(id) else {
            self.panes[idx].source = None;
            return;
        };

        // status line
        ui.horizontal(|ui| {
            if src.busy {
                ui.spinner();
            }
            if let Some(f) = src.status.free_kb {
                ui.label(format!("{} saves · {} KB free", src.rows.len(), f));
            } else {
                ui.label(format!("{} saves", src.rows.len()));
            }
            if !src.status.note.is_empty() {
                ui.colored_label(ui.visuals().warn_fg_color, &src.status.note);
            }
        });
        if let Some(e) = &src.error {
            ui.colored_label(ui.visuals().error_fg_color, e);
        }
        let is_ps2 = matches!(&*src.backend.lock().unwrap(), Backend::Ps2 { .. });
        let mmce_text = src.mmce.clone();
        let n_sel = src.selected.len();

        // toolbar
        let mut action: Option<&'static str> = None;
        ui.horizontal_wrapped(|ui| {
            let arrow = if idx == 0 { "Copy ➡" } else { "⬅ Copy" };
            if ui
                .add_enabled(other.is_some() && n_sel > 0, egui::Button::new(arrow))
                .on_hover_text("Copy selected saves to the other pane")
                .clicked()
            {
                action = Some("copy");
            }
            if ui.add_enabled(n_sel > 0, egui::Button::new("Export…")).clicked() {
                action = Some("export");
            }
            if ui.button("Import…").clicked() {
                action = Some("import");
            }
            if ui
                .add_enabled(n_sel == 1, egui::Button::new("Open"))
                .on_hover_text("Explore the files in this save and upload new ones (or double-click a row)")
                .clicked()
            {
                action = Some("explore");
            }
            if ui.add_enabled(n_sel == 1, egui::Button::new("Rename")).clicked() {
                action = Some("rename");
            }
            if ui.add_enabled(n_sel > 0, egui::Button::new("Delete")).clicked() {
                action = Some("delete");
            }
            if ui
                .button("To MemCard PRO2…")
                .on_hover_text("Upload this whole card, or give each game its own Game ID card on the MemCard PRO2")
                .clicked()
            {
                action = Some("pro2");
            }
            if ui.small_button("All").clicked() {
                action = Some("all");
            }
            if ui.small_button("None").clicked() {
                action = Some("none");
            }
        });
        if is_ps2 {
            ui.horizontal(|ui| {
                ui.label("MMCE:");
                if ui.small_button("◀ card").clicked() {
                    action = Some("mmce_prev");
                }
                if ui.small_button("card ▶").clicked() {
                    action = Some("mmce_next");
                }
                if ui.small_button("◀ ch").clicked() {
                    action = Some("ch_prev");
                }
                if ui.small_button("ch ▶").clicked() {
                    action = Some("ch_next");
                }
                if ui.small_button("?").on_hover_text("Query MemCard PRO2 / SD2PSX status").clicked() {
                    action = Some("mmce_status");
                }
                ui.label(mmce_text);
            });
        }
        ui.add(egui::TextEdit::singleline(&mut self.panes[idx].filter).hint_text("🔍 filter by name or title"));

        let filter = self.panes[idx].filter.to_lowercase();
        let src = self.source(id).unwrap();
        let rows: Vec<SaveRow> = src
            .rows
            .iter()
            .filter(|r| filter.is_empty() || r.name.to_lowercase().contains(&filter) || r.title.to_lowercase().contains(&filter))
            .cloned()
            .collect();
        let selected = src.selected.clone();
        let mut toggle: Option<String> = None;
        let mut details: Option<String> = None;
        let show_fmt = rows.iter().any(|r| !r.format.is_empty());

        let avail = ui.available_height();
        let mut tb = TableBuilder::new(ui)
            .id_salt(("table", idx))
            .striped(true)
            .resizable(true)
            .sense(egui::Sense::click())
            .min_scrolled_height(0.0)
            .max_scroll_height(avail)
            .column(Column::auto().at_least(140.0))
            .column(Column::remainder().at_least(120.0))
            .column(Column::auto().at_least(50.0));
        if show_fmt {
            tb = tb.column(Column::auto());
        }
        tb.header(20.0, |mut h| {
            h.col(|ui| {
                ui.strong("Directory");
            });
            h.col(|ui| {
                ui.strong("Title");
            });
            h.col(|ui| {
                ui.strong("KB");
            });
            if show_fmt {
                h.col(|ui| {
                    ui.strong("Fmt");
                });
            }
        })
        .body(|body| {
            body.rows(20.0, rows.len(), |mut row| {
                let r = &rows[row.index()];
                row.set_selected(selected.contains(&r.key));
                row.col(|ui| {
                    ui.add(egui::Label::new(egui::RichText::new(&r.name).monospace()).truncate().selectable(false));
                });
                row.col(|ui| {
                    ui.add(egui::Label::new(&r.title).truncate().selectable(false));
                });
                row.col(|ui| {
                    ui.label(r.size_kb.to_string());
                });
                if show_fmt {
                    row.col(|ui| {
                        ui.label(&r.format);
                    });
                }
                let resp = row.response();
                if resp.double_clicked() {
                    details = Some(r.key.clone());
                } else if resp.clicked() {
                    toggle = Some(r.key.clone());
                }
                resp.on_hover_text(format!("{}\nmodified {}", r.name, r.modified));
            });
        });

        if let Some(k) = toggle {
            let s = self.source_mut(id).unwrap();
            if !s.selected.remove(&k) {
                s.selected.insert(k);
            }
        }
        if let Some(k) = details {
            self.show_details(id, k);
        }
        match action {
            Some("copy") => {
                if let Some(o) = other {
                    self.copy(id, o)
                }
            }
            Some("export") => self.export_selected(id),
            Some("pro2") => self.open_pro2_dialog(id),
            Some("explore") => {
                if let Some(k) = self.source(id).unwrap().selected.iter().next().cloned() {
                    self.show_details(id, k);
                }
            }
            Some("import") => self.import_files(id),
            Some("delete") => {
                let keys: Vec<String> = self.source(id).unwrap().selected.iter().cloned().collect();
                self.confirm_delete = Some((id, keys));
            }
            Some("rename") => {
                let s = self.source(id).unwrap();
                let key = s.selected.iter().next().cloned().unwrap();
                let name = s.rows.iter().find(|r| r.key == key).map(|r| r.name.clone()).unwrap_or_default();
                self.rename = Some(RenameDialog { source: id, key, new_name: name });
            }
            Some("all") => {
                let keys: Vec<String> = rows.iter().map(|r| r.key.clone()).collect();
                self.source_mut(id).unwrap().selected.extend(keys);
            }
            Some("none") => self.source_mut(id).unwrap().selected.clear(),
            Some("mmce_next") => self.mmce(id, MmceTarget::Card, MmceMode::Next),
            Some("mmce_prev") => self.mmce(id, MmceTarget::Card, MmceMode::Prev),
            Some("ch_next") => self.mmce(id, MmceTarget::Channel, MmceMode::Next),
            Some("ch_prev") => self.mmce(id, MmceTarget::Channel, MmceMode::Prev),
            Some("mmce_status") => {
                let be = self.source(id).unwrap().backend.clone();
                self.job(&[], move |tx| {
                    let t = be.lock().unwrap().mmce_status()?;
                    let _ = tx.send(Msg::Mmce { id, text: t });
                    Ok(())
                });
            }
            _ => {}
        }
    }

    fn open_pro2_dialog(&mut self, id: u64) {
        let Some(src) = self.source(id) else { return };
        let is_ps2_link = matches!(&*src.backend.lock().unwrap(), Backend::Ps2 { .. });
        // name: a single game's ID if the card only holds one game, else the label
        let ids: BTreeSet<String> = src.rows.iter().filter_map(|r| ps2mc::save::game_id_from_dir(&r.name)).collect();
        let folder = if ids.len() == 1 && src.rows.len() == ids.len() {
            ids.into_iter().next().unwrap()
        } else if is_ps2_link {
            src.label.rsplit(' ').next().map(|slot| format!("PS2Slot{slot}")).unwrap_or_else(|| "PS2Slot1".into())
        } else {
            folder_name_from(&src.label)
        };
        let only_selected = !src.selected.is_empty();
        self.pro2 = Some(Pro2Dialog {
            source: id,
            mode: Pro2Mode::PerGame,
            folder,
            channel: 1,
            replace: false,
            only_selected,
            overwrite: false,
            names: true,
        });
    }

    fn pro2_window(&mut self, ctx: &egui::Context) {
        let Some(d) = &mut self.pro2 else { return };
        let Some(src) = self.sources.iter().find(|s| s.id == d.source) else {
            self.pro2 = None;
            return;
        };
        let src_is_ps1 = matches!(&*src.backend.lock().unwrap(), Backend::Ps1 { .. });
        let mut open = true;
        let mut go = false;
        egui::Window::new("Export to MemCard PRO2")
            .id(egui::Id::new("pro2_export"))
            .open(&mut open)
            .default_width(520.0)
            .show(ctx, |ui| {
                ui.label(format!("From: {}", src.label));
                egui::Grid::new("pro2_conn").num_columns(2).show(ui, |ui| {
                    ui.label("MemCard PRO2");
                    ui.add(egui::TextEdit::singleline(&mut self.settings.mcp_host).hint_text("memcardprogc or IP").desired_width(160.0));
                    ui.end_row();
                    ui.label("FTP user");
                    ui.add(egui::TextEdit::singleline(&mut self.settings.mcp_user).desired_width(160.0));
                    ui.end_row();
                    ui.label("Password");
                    ui.add(egui::TextEdit::singleline(&mut self.mcp_password).password(true).desired_width(160.0));
                    ui.end_row();
                });
                ui.separator();
                ui.radio_value(&mut d.mode, Pro2Mode::PerGame, "One card per game (Game ID)");
                ui.radio_value(&mut d.mode, Pro2Mode::WholeCard, "Whole card as one virtual card");
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label("Channel");
                    ui.add(egui::DragValue::new(&mut d.channel).range(1..=8));
                });
                match d.mode {
                    Pro2Mode::WholeCard => {
                        let platform = if src_is_ps1 { Platform::Ps1 } else { Platform::Ps2 };
                        ui.horizontal(|ui| {
                            ui.label("Card name");
                            ui.add(egui::TextEdit::singleline(&mut d.folder).desired_width(200.0));
                        });
                        let ok = ps2net::pro2::valid_folder(&d.folder);
                        if ok {
                            ui.monospace(ps2net::pro2::card_path(platform, &d.folder, d.channel));
                        } else {
                            ui.colored_label(ui.visuals().error_fg_color, "Use letters, digits, - _ . ( ) only");
                        }
                        ui.checkbox(&mut d.replace, "Replace the card if it already exists (old one kept as .bak)");
                        ui.small("Tip: name it after a Game ID (e.g. SLUS-20312) to make it that game's card.");
                        if ui.add_enabled(ok, egui::Button::new("Upload card")).clicked() {
                            go = true;
                        }
                    }
                    Pro2Mode::PerGame => {
                        ui.horizontal(|ui| {
                            ui.radio_value(&mut d.only_selected, true, format!("Selected saves ({})", src.selected.len()));
                            ui.radio_value(&mut d.only_selected, false, format!("All saves ({})", src.rows.len()));
                        });
                        // plan
                        let rows: Vec<&SaveRow> =
                            src.rows.iter().filter(|r| !d.only_selected || src.selected.contains(&r.key)).collect();
                        let mut plan: std::collections::BTreeMap<String, Vec<&str>> = Default::default();
                        let mut unmatched = Vec::new();
                        for r in &rows {
                            let platform = if src_is_ps1 || r.format == "mcs" || r.format == "PS1" { Platform::Ps1 } else { Platform::Ps2 };
                            match ps2mc::save::game_id_from_dir(&r.name) {
                                Some(id) => plan.entry(ps2net::pro2::card_path(platform, &id, d.channel)).or_default().push(&r.name),
                                None => unmatched.push(r.name.as_str()),
                            }
                        }
                        egui::ScrollArea::vertical().max_height(200.0).show(ui, |ui| {
                            egui::Grid::new("pro2_plan").striped(true).num_columns(2).show(ui, |ui| {
                                for (path, names) in &plan {
                                    ui.monospace(path);
                                    ui.label(names.join(", "));
                                    ui.end_row();
                                }
                            });
                        });
                        if !unmatched.is_empty() {
                            ui.colored_label(
                                ui.visuals().warn_fg_color,
                                format!("No Game ID, skipped: {}", unmatched.join(", ")),
                            );
                        }
                        ui.checkbox(&mut d.overwrite, "Replace saves already on a game's card");
                        ui.checkbox(&mut d.names, "Write name.txt with the game title");
                        ui.small("Existing Game ID cards are updated, not replaced; the previous file is kept as .bak.");
                        if ui.add_enabled(!plan.is_empty(), egui::Button::new(format!("Upload {} card(s)", plan.len()))).clicked() {
                            go = true;
                        }
                    }
                }
            });
        if !open {
            self.pro2 = None;
            return;
        }
        if !go {
            return;
        }
        self.settings.save();
        let d = self.pro2.take().unwrap();
        let src = self.source(d.source).unwrap();
        let be = src.backend.clone();
        let keys: Vec<String> = if d.only_selected {
            src.selected.iter().cloned().collect()
        } else {
            src.rows.iter().map(|r| r.key.clone()).collect()
        };
        let (host, user, pass) = (self.settings.mcp_host.clone(), self.settings.mcp_user.clone(), self.mcp_password.clone());
        self.job(&[d.source], move |tx| {
            match d.mode {
                Pro2Mode::WholeCard => {
                    let log = tx.clone();
                    let (platform, image) = be.lock().unwrap().card_image(|m| {
                        let _ = log.send(Msg::Log(m));
                    })?;
                    let _ = tx.send(Msg::Log(format!("uploading {} KB to the MemCard PRO2…", image.len() / 1024)));
                    let mut m = MemcardPro::connect(&host, &user, &pass)?;
                    let path = ps2net::pro2::export_whole_card(&mut m, platform, &d.folder, d.channel, &image, d.replace)?;
                    m.quit();
                    let _ = tx.send(Msg::Log(format!("MemCard PRO2: wrote {path}")));
                }
                Pro2Mode::PerGame => {
                    let mut saves: Vec<AnySave> = Vec::new();
                    for k in &keys {
                        saves.push(be.lock().unwrap().get_any(k)?);
                    }
                    let groups = ps2mc::anysave::group_by_game(saves);
                    let mut m = MemcardPro::connect(&host, &user, &pass)?;
                    let log = tx.clone();
                    let rep = ps2net::pro2::export_per_game(&mut m, &groups, d.channel, d.overwrite, d.names, |i, n, id| {
                        if !id.is_empty() {
                            let _ = log.send(Msg::Log(format!("MemCard PRO2: {id} ({}/{n})", i + 1)));
                        }
                    })?;
                    m.quit();
                    for p in &rep.created {
                        let _ = tx.send(Msg::Log(format!("  new card {p}")));
                    }
                    for p in &rep.updated {
                        let _ = tx.send(Msg::Log(format!("  updated {p}")));
                    }
                    for k in &rep.kept {
                        let _ = tx.send(Msg::Log(format!("  {k} was already on its card (not replaced)")));
                    }
                    for (id, e) in &rep.failed {
                        let _ = tx.send(Msg::Error(format!("  {id}: {e}")));
                    }
                    let _ = tx.send(Msg::Log(format!("MemCard PRO2 export done: {}", rep.summary())));
                }
            }
            Ok(())
        });
    }

    fn explorer_window(&mut self, ctx: &egui::Context) {
        let Some(ex) = &mut self.explorer else { return };
        let editable = ex.source.is_some();
        let source_label = ex.source.and_then(|id| self.sources.iter().find(|s| s.id == id)).map(|s| s.label.clone());
        let mut open = true;
        // actions collected while drawing, run afterwards
        let mut upload = false;
        let mut download = false;
        let mut delete: Option<Vec<String>> = None;
        let mut rename: Option<(String, String)> = None;
        let mut save_as: Option<SaveFormat> = None;

        egui::Window::new(format!("Save folder: {}", ex.save.name))
            .id(egui::Id::new("explorer"))
            .open(&mut open)
            .default_width(560.0)
            .default_height(480.0)
            .show(ctx, |ui| {
                let save = &ex.save;
                ui.horizontal_wrapped(|ui| {
                    ui.strong(save.title());
                    if let Some(id) = save.game_id() {
                        ui.label(format!("· {id} {}", save.region()));
                    }
                });
                ui.label(format!(
                    "{} file(s) · {} KB on card · modified {}",
                    save.files.len(),
                    save.card_size_kb(),
                    save.modified
                ));
                match &source_label {
                    Some(l) => ui.small(format!("in {l}")),
                    None => ui.colored_label(ui.visuals().warn_fg_color, "Opened from a file: read-only. Import it into a card to edit."),
                };
                ui.separator();

                // toolbar
                let n_sel = ex.selected.len();
                ui.horizontal_wrapped(|ui| {
                    ui.add_enabled_ui(editable && !ex.busy, |ui| {
                        if ui.button("⬆ Upload files…").on_hover_text("Add files to this save folder (or drop them on the window)").clicked() {
                            upload = true;
                        }
                    });
                    if ui.add_enabled(n_sel > 0, egui::Button::new("⬇ Download…")).clicked() {
                        download = true;
                    }
                    ui.add_enabled_ui(editable && !ex.busy, |ui| {
                        if ui.add_enabled(n_sel == 1, egui::Button::new("Rename")).clicked() {
                            let n = ex.selected.iter().next().cloned().unwrap();
                            ex.rename = Some((n.clone(), n));
                        }
                        if ui.add_enabled(n_sel > 0, egui::Button::new("Delete")).clicked() {
                            ex.confirm_delete = true;
                        }
                    });
                    ui.add_enabled(editable, egui::Checkbox::new(&mut ex.replace, "Replace existing files"));
                    if ex.busy {
                        ui.spinner();
                    }
                });

                if let Some((from, to)) = &mut ex.rename {
                    let mut go = false;
                    let mut cancel = false;
                    ui.horizontal(|ui| {
                        ui.label(format!("Rename {from} to"));
                        let r = ui.add(egui::TextEdit::singleline(to).char_limit(31).desired_width(180.0));
                        if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            go = true;
                        }
                        if ui.button("OK").clicked() {
                            go = true;
                        }
                        if ui.button("Cancel").clicked() {
                            cancel = true;
                        }
                    });
                    if go && from != to {
                        rename = Some((from.clone(), to.clone()));
                    }
                    if go || cancel {
                        ex.rename = None;
                    }
                }
                if ex.confirm_delete {
                    ui.horizontal(|ui| {
                        ui.colored_label(ui.visuals().warn_fg_color, format!("Delete {} file(s) from this save?", n_sel));
                        if ui.button("Delete").clicked() {
                            delete = Some(ex.selected.iter().cloned().collect());
                            ex.confirm_delete = false;
                        }
                        if ui.button("Cancel").clicked() {
                            ex.confirm_delete = false;
                        }
                    });
                }

                // file table
                let mut toggle: Option<String> = None;
                let files = &ex.save.files;
                let table_h = (ui.available_height() - 60.0).max(120.0);
                TableBuilder::new(ui)
                    .id_salt("explorer_files")
                    .striped(true)
                    .resizable(true)
                    .sense(egui::Sense::click())
                    .min_scrolled_height(0.0)
                    .max_scroll_height(table_h)
                    .column(Column::auto().at_least(200.0))
                    .column(Column::auto().at_least(80.0))
                    .column(Column::remainder())
                    .header(20.0, |mut h| {
                        h.col(|ui| {
                            ui.strong("File");
                        });
                        h.col(|ui| {
                            ui.strong("Bytes");
                        });
                        h.col(|ui| {
                            ui.strong("Modified");
                        });
                    })
                    .body(|body| {
                        body.rows(20.0, files.len(), |mut row| {
                            let f = &files[row.index()];
                            row.set_selected(ex.selected.contains(&f.name));
                            row.col(|ui| {
                                ui.add(egui::Label::new(egui::RichText::new(&f.name).monospace()).selectable(false));
                            });
                            row.col(|ui| {
                                ui.label(f.data.len().to_string());
                            });
                            row.col(|ui| {
                                ui.label(f.modified.to_string());
                            });
                            if row.response().clicked() {
                                toggle = Some(f.name.clone());
                            }
                        });
                    });
                if let Some(n) = toggle {
                    if !ex.selected.remove(&n) {
                        ex.selected.insert(n);
                    }
                }

                ui.separator();
                if editable {
                    ui.small("Drop files onto this window to upload them. Games expect their own file names — replacing icon.sys or the data file changes the save.");
                }
                ui.horizontal(|ui| {
                    ui.label("Export whole save:");
                    for fmt in SaveFormat::ALL {
                        if ui.small_button(format!(".{}", fmt.extension())).clicked() {
                            save_as = Some(fmt);
                        }
                    }
                });
            });

        if !open {
            self.explorer = None;
            return;
        }
        let ex = self.explorer.as_ref().unwrap();
        let (id, key) = (ex.source, ex.key.clone());

        if download {
            if let Some(dir) = rfd::FileDialog::new().set_title("Download files to folder").pick_folder() {
                for f in ex.save.files.iter().filter(|f| ex.selected.contains(&f.name)) {
                    let p = dir.join(&f.name);
                    let r = std::fs::write(&p, &f.data);
                    let _ = self.tx.send(match r {
                        Ok(()) => Msg::Log(format!("saved {}", p.display())),
                        Err(e) => Msg::Error(format!("{}: {e}", p.display())),
                    });
                }
            }
        }
        if let Some(fmt) = save_as {
            if let Some(p) = rfd::FileDialog::new().set_file_name(ex.save.suggested_filename(fmt.extension())).save_file() {
                let r = fmt.save(&ex.save).map_err(anyhow::Error::from).and_then(|b| Ok(std::fs::write(&p, b)?));
                let _ = self.tx.send(match r {
                    Ok(()) => Msg::Log(format!("wrote {}", p.display())),
                    Err(e) => Msg::Error(e.to_string()),
                });
            }
        }
        if upload {
            if let Some(files) = rfd::FileDialog::new().set_title(format!("Upload files into {key}")).pick_files() {
                self.upload_into_save(files);
            }
        }
        if let (Some(id), Some(names)) = (id, delete) {
            self.explorer_job(id, key.clone(), move |b| {
                b.delete_files(&key, &names)?;
                Ok(Some(format!("deleted {} from {key}", names.join(", "))))
            });
        } else if let (Some(id), Some((from, to))) = (id, rename) {
            self.explorer_job(id, key.clone(), move |b| {
                b.rename_file(&key, &from, &to)?;
                Ok(Some(format!("renamed {key}/{from} → {to}")))
            });
        }
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        self.explorer_window(ctx);
        self.pro2_window(ctx);

        if let Some(d) = &mut self.rename {
            let mut open = true;
            let mut go = false;
            egui::Window::new("Rename save").open(&mut open).collapsible(false).show(ctx, |ui| {
                ui.label(format!("Rename {}", d.key));
                let r = ui.add(egui::TextEdit::singleline(&mut d.new_name).char_limit(31));
                if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    go = true;
                }
                ui.small("Up to 31 ASCII characters. Games find their saves by this name.");
                if ui.button("Rename").clicked() {
                    go = true;
                }
            });
            if go {
                let (id, key, name) = (d.source, d.key.clone(), d.new_name.clone());
                self.rename = None;
                if let Some(src) = self.source(id) {
                    let be = src.backend.clone();
                    self.job(&[id], move |tx| {
                        be.lock().unwrap().rename(&key, &name)?;
                        let _ = tx.send(Msg::Log(format!("renamed {key} → {name}")));
                        Ok(())
                    });
                }
            } else if !open {
                self.rename = None;
            }
        }

        if let Some((id, keys)) = self.confirm_delete.clone() {
            let mut close = false;
            egui::Window::new("Delete saves?").collapsible(false).resizable(false).show(ctx, |ui| {
                ui.label(format!("Permanently delete {} save(s)?", keys.len()));
                for k in keys.iter().take(8) {
                    ui.monospace(k);
                }
                if keys.len() > 8 {
                    ui.label(format!("… and {} more", keys.len() - 8));
                }
                ui.horizontal(|ui| {
                    if ui.button("Delete").clicked() {
                        self.delete(id, keys.clone());
                        close = true;
                    }
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                });
            });
            if close {
                self.confirm_delete = None;
            }
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.handle_messages();

        // dropped files: into the open save explorer if there is one,
        // otherwise open them as cards / save folders
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().filter_map(|f| f.path.clone()).collect());
        if !dropped.is_empty() && self.explorer.as_ref().is_some_and(|e| e.source.is_some()) {
            self.upload_into_save(dropped);
        } else {
            for p in dropped {
            if p.is_dir() {
                self.add_library(p);
            } else {
                self.open_path(p);
            }
            }
        }

        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.strong("PS2 Save Manager");
                ui.separator();
                ui.checkbox(&mut self.overwrite, "Overwrite existing saves");
                ui.separator();
                ui.label("Export format");
                let before = self.export_format;
                egui::ComboBox::from_id_salt("fmt").selected_text(format!(".{}", self.export_format.extension())).show_ui(ui, |ui| {
                    for f in SaveFormat::ALL {
                        ui.selectable_value(&mut self.export_format, f, f.description());
                    }
                });
                if before != self.export_format {
                    self.settings.export_format = self.export_format.extension().into();
                    self.settings.save();
                    for s in &self.sources {
                        if let Backend::Library { format, .. } = &mut *s.backend.lock().unwrap() {
                            *format = self.export_format;
                        }
                    }
                }
            });
        });

        egui::TopBottomPanel::bottom("log").resizable(true).default_height(90.0).show(ctx, |ui| {
            egui::ScrollArea::vertical().stick_to_bottom(true).auto_shrink([false, false]).show(ui, |ui| {
                for (is_err, l) in &self.log {
                    if *is_err {
                        ui.colored_label(ui.visuals().error_fg_color, l);
                    } else {
                        ui.label(l);
                    }
                }
            });
        });

        egui::SidePanel::left("sources").default_width(260.0).show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| self.sidebar(ui));
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.columns(2, |cols| {
                self.pane(&mut cols[0], 0);
                self.pane(&mut cols[1], 1);
            });
        });

        self.dialogs(ctx);
    }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("PS2 Save Manager")
            .with_inner_size([1200.0, 760.0])
            .with_min_inner_size([800.0, 500.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native("PS2 Save Manager", options, Box::new(|cc| Ok(Box::new(App::new(cc)))))
}
