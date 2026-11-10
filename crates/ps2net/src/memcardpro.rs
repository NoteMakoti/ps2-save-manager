//! MemCard PRO2 (8BitMods) over Wi-Fi.
//!
//! The MemCard PRO2 exposes its microSD card through a plain FTP server
//! (enable it in the device's WebUI → Settings, set a user/password and
//! power-cycle). Virtual PS2 cards are stored as raw `.mc2` images (8 MB
//! card + ECC = 8,650,752 bytes, or larger sizes) under the `PS2` folder,
//! one folder per card/GameID with one file per channel.
//!
//! The device only supports one FTP connection, plain FTP, passive mode —
//! that is what this client uses. Rather than hard-coding the folder
//! layout (which has changed between firmware versions) we discover
//! cards by walking the tree and picking up every `.mc2` file.
//!
//! Do not write a card that is currently mounted by the console; switch
//! the PRO2 to another card or turn the console off first.

use std::io::Cursor;
use std::time::Duration;

use suppaftp::list::File as FtpFile;
use suppaftp::types::FileType;
use suppaftp::{FtpStream, Mode};

use crate::{NetError, NetResult};

fn ftp_err(e: suppaftp::FtpError) -> NetError {
    NetError::Ftp(e.to_string())
}

#[derive(Debug, Clone)]
pub struct RemoteFile {
    pub path: String,
    pub name: String,
    pub size: u64,
    pub is_dir: bool,
}

/// A virtual memory card found on the device.
#[derive(Debug, Clone)]
pub struct VirtualCard {
    /// Full path, e.g. `/PS2/SLUS-20312/SLUS-20312-1.mc2`
    pub path: String,
    /// Containing folder name (card name or GameID).
    pub group: String,
    pub file_name: String,
    pub size: u64,
    /// `.mc2` cards are PS2, `.mcd` cards PS1.
    pub platform: ps2mc::Platform,
}

impl VirtualCard {
    /// Channel number parsed from a `NAME-<n>.mc2` file name.
    pub fn channel(&self) -> Option<u32> {
        let stem = self.file_name.get(..self.file_name.len().checked_sub(4)?)?;
        stem.rsplit('-').next()?.parse().ok()
    }
}

pub struct MemcardPro {
    ftp: FtpStream,
    pub welcome: Option<String>,
}

fn join(dir: &str, name: &str) -> String {
    if dir.ends_with('/') { format!("{dir}{name}") } else { format!("{dir}/{name}") }
}

impl MemcardPro {
    /// Connect and log in. `host` may include a port (default 21).
    pub fn connect(host: &str, user: &str, password: &str) -> NetResult<MemcardPro> {
        let addr = if host.contains(':') { host.to_string() } else { format!("{host}:21") };
        let sa = std::net::ToSocketAddrs::to_socket_addrs(&addr)?
            .next()
            .ok_or_else(|| NetError::Ftp(format!("cannot resolve {addr}")))?;
        let mut ftp = FtpStream::connect_timeout(sa, Duration::from_secs(8)).map_err(ftp_err)?;
        ftp.get_ref().set_read_timeout(Some(Duration::from_secs(60))).ok();
        let welcome = ftp.get_welcome_msg().map(str::to_string);
        ftp.login(user, password).map_err(ftp_err)?;
        ftp.set_mode(Mode::Passive);
        ftp.transfer_type(FileType::Binary).map_err(ftp_err)?;
        Ok(MemcardPro { ftp, welcome })
    }

    /// List a remote directory.
    pub fn list(&mut self, dir: &str) -> NetResult<Vec<RemoteFile>> {
        let lines = self.ftp.list(Some(dir)).map_err(ftp_err)?;
        let mut out = Vec::new();
        for l in &lines {
            if let Ok(f) = FtpFile::try_from(l.as_str()) {
                let name = f.name().to_string();
                if name == "." || name == ".." {
                    continue;
                }
                out.push(RemoteFile { path: join(dir, &name), name, size: f.size() as u64, is_dir: f.is_directory() });
            }
        }
        if out.is_empty() && !lines.is_empty() {
            // unparseable LIST output: fall back to NLST
            for n in self.ftp.nlst(Some(dir)).map_err(ftp_err)? {
                let name = n.rsplit('/').next().unwrap_or(&n).to_string();
                let is_dir = !name.contains('.');
                out.push(RemoteFile { path: join(dir, &name), name, size: 0, is_dir });
            }
        }
        out.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then(a.name.to_lowercase().cmp(&b.name.to_lowercase())));
        Ok(out)
    }

    /// Find virtual cards (`.mc2` PS2, `.mcd` PS1). Looks under `/PS2`,
    /// `/PS1` and `/MemoryCards` (original MemCard PRO), else the root.
    pub fn find_cards(&mut self) -> NetResult<Vec<VirtualCard>> {
        let mut cards = Vec::new();
        let entries = self.list("/")?;
        let mut roots: Vec<String> = entries
            .iter()
            .filter(|e| {
                e.is_dir && ["PS2", "PS1", "MemoryCards"].iter().any(|n| e.name.eq_ignore_ascii_case(n))
            })
            .map(|e| e.path.clone())
            .collect();
        if roots.is_empty() {
            roots.push("/".into());
        }
        for r in roots {
            self.walk(&r, 0, &mut cards)?;
        }
        cards.sort_by(|a, b| a.path.to_lowercase().cmp(&b.path.to_lowercase()));
        Ok(cards)
    }

    fn walk(&mut self, dir: &str, depth: usize, out: &mut Vec<VirtualCard>) -> NetResult<()> {
        if depth > 3 {
            return Ok(());
        }
        for e in self.list(dir)? {
            if e.is_dir {
                if !e.name.eq_ignore_ascii_case("Templates") {
                    self.walk(&e.path, depth + 1, out)?;
                }
            } else {
                let lower = e.name.to_ascii_lowercase();
                let platform = if lower.ends_with(".mc2") {
                    ps2mc::Platform::Ps2
                } else if lower.ends_with(".mcd") {
                    ps2mc::Platform::Ps1
                } else {
                    continue;
                };
                let group = dir.trim_end_matches('/').rsplit('/').next().unwrap_or("").to_string();
                out.push(VirtualCard { path: e.path, group, file_name: e.name, size: e.size, platform });
            }
        }
        Ok(())
    }

    pub fn download(&mut self, path: &str) -> NetResult<Vec<u8>> {
        let buf = self.ftp.retr_as_buffer(path).map_err(ftp_err)?;
        Ok(buf.into_inner())
    }

    /// Upload `data` to `path`. With `keep_backup` the existing file is
    /// first renamed to `<path>.bak` (replacing an older backup).
    pub fn upload(&mut self, path: &str, data: &[u8], keep_backup: bool) -> NetResult<()> {
        if keep_backup {
            let bak = format!("{path}.bak");
            let _ = self.ftp.rm(&bak);
            let _ = self.ftp.rename(path, &bak);
        }
        let mut cur = Cursor::new(data);
        let n = self.ftp.put_file(path, &mut cur).map_err(ftp_err)?;
        if n as usize != data.len() {
            return Err(NetError::Ftp(format!("short upload: {n} of {} bytes", data.len())));
        }
        Ok(())
    }

    pub fn mkdir(&mut self, path: &str) -> NetResult<()> {
        self.ftp.mkdir(path).map_err(ftp_err)
    }

    pub fn quit(mut self) {
        let _ = self.ftp.quit();
    }
}
