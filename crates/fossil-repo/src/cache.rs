use std::path::PathBuf;

use sha2::{Digest, Sha256};

use fossil_core::storage::{RepoStore, cache_root, index_db_path};
use fossil_core::types::RepoMeta;

use crate::error::RepoError;

/// Derives the 16-hex-char repo_id from a canonical repository URL.
pub fn repo_id_from_url(url: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(url.as_bytes());
    let hash = hasher.finalize();
    hex::encode(&hash[..8]) // 8 bytes → 16 hex chars
}

/// Manages the on-disk cache layout for cloned repositories.
pub struct CacheManager;

impl CacheManager {
    /// Returns the directory for a given repo_id, without creating it.
    pub fn repo_dir(repo_id: &str) -> PathBuf {
        cache_root().join(repo_id)
    }

    /// Returns true if the repo directory already exists on disk.
    pub fn is_cached(repo_id: &str) -> bool {
        Self::repo_dir(repo_id).exists()
    }

    /// Ensure the cache root exists.
    pub fn ensure_root() -> Result<(), RepoError> {
        std::fs::create_dir_all(cache_root())?;
        Ok(())
    }

    /// Remove a repo's cache directory entirely.
    pub fn remove(repo_id: &str) -> Result<(), RepoError> {
        let dir = Self::repo_dir(repo_id);
        if dir.exists() {
            std::fs::remove_dir_all(&dir)?;
        }
        Ok(())
    }

    /// Open (or create) the SQLite index for this repo.
    pub fn open_store(repo_id: &str) -> Result<RepoStore, RepoError> {
        let db_path = index_db_path(&Self::repo_dir(repo_id));
        let store = RepoStore::open(&db_path)?;
        Ok(store)
    }

    /// Return metadata for every cached (and indexed) repo.
    pub fn list_all() -> Result<Vec<RepoMeta>, RepoError> {
        let root = cache_root();
        if !root.exists() {
            return Ok(vec![]);
        }

        let mut result = Vec::new();
        for entry in std::fs::read_dir(root)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let db_path = index_db_path(&entry.path());
            if !db_path.exists() {
                continue;
            }
            match RepoStore::open(&db_path) {
                Ok(store) => {
                    let repo_id = entry.file_name().to_string_lossy().to_string();
                    match store.get_repo(&repo_id) {
                        Ok(meta) => result.push(meta),
                        Err(_) => {} // DB exists but no repo row yet — skip
                    }
                }
                Err(_) => {} // corrupted DB — skip
            }
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_id_is_16_chars() {
        let id = repo_id_from_url("https://github.com/tokio-rs/tokio");
        assert_eq!(id.len(), 16);
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn same_url_gives_same_id() {
        let url = "https://github.com/rust-lang/rust";
        assert_eq!(repo_id_from_url(url), repo_id_from_url(url));
    }

    #[test]
    fn different_urls_give_different_ids() {
        let a = repo_id_from_url("https://github.com/tokio-rs/tokio");
        let b = repo_id_from_url("https://github.com/serde-rs/serde");
        assert_ne!(a, b);
    }
}
