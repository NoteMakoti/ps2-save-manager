//! Tiny key=value settings file (no passwords are stored).

use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Settings {
    pub ps2_host: String,
    pub mcp_host: String,
    pub mcp_user: String,
    pub library_dir: Option<PathBuf>,
    pub export_format: String,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            ps2_host: String::new(),
            mcp_host: "memcardprogc".into(),
            mcp_user: "admin".into(),
            library_dir: None,
            export_format: "psu".into(),
        }
    }
}

fn path() -> Option<PathBuf> {
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support"))
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
    };
    base.map(|b| b.join("ps2sm").join("gui.conf"))
}

impl Settings {
    pub fn load() -> Settings {
        let mut s = Settings::default();
        let Some(p) = path() else { return s };
        let Ok(text) = std::fs::read_to_string(p) else { return s };
        for line in text.lines() {
            if let Some((k, v)) = line.split_once('=') {
                let v = v.trim().to_string();
                match k.trim() {
                    "ps2_host" => s.ps2_host = v,
                    "mcp_host" => s.mcp_host = v,
                    "mcp_user" => s.mcp_user = v,
                    "library_dir" if !v.is_empty() => s.library_dir = Some(PathBuf::from(v)),
                    "export_format" => s.export_format = v,
                    _ => {}
                }
            }
        }
        s
    }

    pub fn save(&self) {
        let Some(p) = path() else { return };
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let text = format!(
            "ps2_host={}\nmcp_host={}\nmcp_user={}\nlibrary_dir={}\nexport_format={}\n",
            self.ps2_host,
            self.mcp_host,
            self.mcp_user,
            self.library_dir.as_ref().map(|p| p.display().to_string()).unwrap_or_default(),
            self.export_format
        );
        let _ = std::fs::write(p, text);
    }
}
