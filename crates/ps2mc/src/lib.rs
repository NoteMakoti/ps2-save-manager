//! `ps2mc` — read, edit and create PlayStation 2 memory card images and
//! convert individual saves between the common exchange formats.
//!
//! * [`card::MemoryCard`] — a full card image (`.ps2`, `.mc2`, `.bin`,
//!   `.vmc`), with or without per-page ECC data, 8 MB up to 128 MB.
//! * [`save::Save`] — one save (a top level directory with its files),
//!   the unit that is copied between cards, PCs and the PS2.
//! * [`formats`] — `.psu` (EMS/uLaunchELF), `.max` (Action Replay MAX),
//!   `.cbs` (CodeBreaker) and `.sps`/`.xps` (SharkPort / X-Port).
//! * [`iconsys::IconSys`] — the `icon.sys` metadata (save title).

pub mod anysave;
pub mod card;
pub mod dirent;
pub mod ecc;
pub mod error;
pub mod formats;
pub mod iconsys;
pub mod ps1;
pub mod save;
pub mod tod;

pub use anysave::{AnySave, Platform};
pub use card::{CardInfo, MemoryCard};
pub use ps1::{Ps1Card, Ps1Save};
pub use dirent::{DirEntry, Mode};
pub use error::{Error, Result};
pub use formats::SaveFormat;
pub use save::{Save, SaveFile};
pub use tod::Tod;
