use thiserror::Error;

/// Top-level errors for fossil-core.
#[derive(Debug, Error)]
pub enum CoreError {
    #[error("invalid symbol kind: {0}")]
    InvalidSymbolKind(String),

    #[error("storage error: {0}")]
    Storage(#[from] StorageError),
}

/// Errors that originate from the SQLite storage layer.
#[derive(Debug, Error)]
pub enum StorageError {
    #[error("database error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("repository not found: {0}")]
    RepoNotFound(String),

    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}
