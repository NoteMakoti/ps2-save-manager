use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("not a PS2 memory card image: {0}")]
    NotACard(String),
    #[error("corrupt memory card: {0}")]
    Corrupt(String),
    #[error("corrupt save file: {0}")]
    CorruptSave(String),
    #[error("unsupported or unrecognised save format")]
    UnknownFormat,
    #[error("not found: {0}")]
    NotFound(String),
    #[error("already exists: {0}")]
    Exists(String),
    #[error("not a directory: {0}")]
    NotADirectory(String),
    #[error("is a directory: {0}")]
    IsADirectory(String),
    #[error("directory not empty: {0}")]
    NotEmpty(String),
    #[error("memory card is full (need {needed} KB, {free} KB free)")]
    NoSpace { needed: u64, free: u64 },
    #[error("invalid name: {0}")]
    InvalidName(String),
    #[error("invalid argument: {0}")]
    Invalid(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
