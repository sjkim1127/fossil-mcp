use thiserror::Error;

/// Errors produced by fossil-repo operations.
#[derive(Debug, Error)]
pub enum RepoError {
    #[error("git error: {0}")]
    Git(#[from] git2::Error),

    #[error("clone failed for '{url}': {source}")]
    CloneFailed {
        url: String,
        #[source]
        source: git2::Error,
    },

    #[error("repository not found in cache: {0}")]
    NotFound(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("storage error: {0}")]
    Storage(#[from] fossil_core::error::StorageError),
}
