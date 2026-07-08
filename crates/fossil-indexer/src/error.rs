use thiserror::Error;

/// Errors produced by the indexer.
#[derive(Debug, Error)]
pub enum IndexError {
    #[error("unsupported language for file: {0}")]
    UnsupportedLanguage(String),

    #[error("parse error in file '{file}': {message}")]
    ParseFailed { file: String, message: String },

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("storage error: {0}")]
    Storage(#[from] fossil_core::error::StorageError),
}
