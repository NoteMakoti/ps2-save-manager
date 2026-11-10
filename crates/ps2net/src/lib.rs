//! Network access to PS2 memory cards.
//!
//! * [`client::Ps2Client`] — talks to the PS2SM homebrew app running on a
//!   PS2 (protocol in `docs/PROTOCOL.md`).
//! * [`server`] — a server for the same protocol backed by card image
//!   files; used for tests, demos and to share a local card over the LAN.
//! * [`memcardpro::MemcardPro`] — browses and transfers the virtual
//!   memory cards (`.mc2`) on a MemCard PRO2 over its FTP server.

pub mod client;
pub mod memcardpro;
pub mod pro2;
pub mod proto;
pub mod server;

pub use client::Ps2Client;
pub use memcardpro::MemcardPro;

#[derive(Debug, thiserror::Error)]
pub enum NetError {
    #[error("network error: {0}")]
    Io(#[from] std::io::Error),
    #[error("protocol error: {0}")]
    Protocol(String),
    #[error("PS2 reported error {code}: {message}")]
    Remote { code: i16, message: String },
    #[error("{0}")]
    Refused(String),
    #[error("FTP error: {0}")]
    Ftp(String),
    #[error(transparent)]
    Card(#[from] ps2mc::Error),
}

pub type NetResult<T> = Result<T, NetError>;
