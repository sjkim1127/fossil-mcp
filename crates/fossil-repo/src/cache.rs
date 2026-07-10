use std::path::PathBuf;

use sha2::{Digest, Sha256};

use fossil_core::storage::{GlobalStore, cache_root, global_db_path};
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

    /// Open (or create) the global SQLite index store.
    pub fn global_store() -> Result<GlobalStore, RepoError> {
        Self::ensure_root()?;
        let db_path = global_db_path();
        let store = GlobalStore::open(&db_path)?;
        Ok(store)
    }

    /// Return metadata for every cached (and indexed) repo.
    pub fn list_all() -> Result<Vec<RepoMeta>, RepoError> {
        let store = Self::global_store()?;
        store.list_repos().map_err(RepoError::Storage)
    }

    /// Recursively calculates the size of a directory.
    fn get_dir_size(path: &std::path::Path) -> std::io::Result<u64> {
        let mut size = 0;
        if path.is_dir() {
            for entry in std::fs::read_dir(path)? {
                let entry = entry?;
                let metadata = entry.metadata()?;
                if metadata.is_dir() {
                    size += Self::get_dir_size(&entry.path())?;
                } else {
                    size += metadata.len();
                }
            }
        } else {
            size += path.metadata()?.len();
        }
        Ok(size)
    }

    /// Enforces the capacity limit on the cache by evicting the oldest accessed repositories.
    pub fn enforce_capacity(store: &GlobalStore, max_bytes: u64) -> Result<(), RepoError> {
        let root = cache_root();
        if !root.exists() {
            return Ok(());
        }

        loop {
            let current_size = Self::get_dir_size(&root).unwrap_or(0);
            if current_size <= max_bytes {
                break;
            }

            let mut repos = store.list_repos().map_err(RepoError::Storage)?;
            if repos.is_empty() {
                break; // Nothing left to evict
            }

            // Sort by last_accessed_at (oldest first).
            // Repos without a last_accessed_at are considered oldest (e.g. legacy repos)
            repos.sort_by(|a, b| match (a.last_accessed_at, b.last_accessed_at) {
                (Some(a_time), Some(b_time)) => a_time.cmp(&b_time),
                (None, Some(_)) => std::cmp::Ordering::Less,
                (Some(_), None) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            });

            let oldest_repo = &repos[0];
            tracing::info!(
                "Cache size {} exceeds limit {}. Evicting repo '{}' ({})",
                current_size,
                max_bytes,
                oldest_repo.url,
                oldest_repo.repo_id
            );

            // 1. Delete from DB
            if let Err(e) = store.evict_repo(&oldest_repo.repo_id) {
                tracing::warn!(
                    "Failed to evict repo {} from DB: {}",
                    oldest_repo.repo_id,
                    e
                );
            }

            // 2. Delete physical clone dir
            let repo_dir = Self::repo_dir(&oldest_repo.repo_id);
            if repo_dir.exists()
                && let Err(e) = std::fs::remove_dir_all(&repo_dir)
            {
                tracing::warn!("Failed to remove directory {:?}: {}", repo_dir, e);
            }

            // Re-calculate on next loop iteration until under max_bytes
        }

        Ok(())
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
