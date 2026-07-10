use std::path::Path;

use git2::FetchOptions;
use tracing::{debug, info};

use chrono::Utc;
use fossil_core::types::RepoMeta;

use crate::cache::{CacheManager, repo_id_from_url};
use crate::error::RepoError;

/// Options controlling how a repository is cloned.
pub struct CloneOptions<'a> {
    pub url: &'a str,
    pub alias: Option<String>,
    pub branch: Option<String>,
    /// If true, delete the existing cache directory and re-clone.
    pub refresh: bool,
}

/// Clone a public repository (shallow, depth=1) into the fossil-mcp cache.
///
/// If the repo is already cached and `refresh` is false, the existing clone is
/// reused and the function returns immediately with the cached metadata.
pub fn clone_repo(opts: CloneOptions<'_>) -> Result<RepoMeta, RepoError> {
    CacheManager::ensure_root()?;

    let repo_id = repo_id_from_url(opts.url);
    let repo_dir = CacheManager::repo_dir(&repo_id);

    // Handle refresh: blow away existing cache.
    if opts.refresh && repo_dir.exists() {
        info!("Refreshing cache for repo_id={}", repo_id);
        CacheManager::remove(&repo_id)?;
    }

    // Reuse existing clone.
    if repo_dir.exists() {
        debug!("Cache hit for repo_id={} at {:?}", repo_id, repo_dir);
        let store = CacheManager::global_store()?;
        return store
            .get_repo(&repo_id)
            .map_err(RepoError::Storage)
            .or_else(|_| {
                // DB exists but no row yet — synthesise metadata.
                Ok(RepoMeta {
                    repo_id: repo_id.to_string(),
                    url: opts.url.to_string(),
                    alias: opts.alias.clone(),
                    path: repo_dir,
                    indexed_at: None,
                    last_accessed_at: Some(Utc::now()),
                    symbol_count: 0,
                })
            });
    }

    // Fresh clone.
    info!(
        "Cloning {} (branch={:?}) → {:?}",
        opts.url, opts.branch, repo_dir
    );
    std::fs::create_dir_all(&repo_dir)?;

    do_shallow_clone(opts.url, opts.branch.as_deref(), &repo_dir).map_err(|e| {
        // Clean up the partially-created directory on failure.
        let _ = std::fs::remove_dir_all(&repo_dir);
        RepoError::CloneFailed {
            url: opts.url.to_string(),
            source: e,
        }
    })?;

    let meta = RepoMeta {
        repo_id: repo_id.to_string(),
        url: opts.url.to_string(),
        alias: opts.alias,
        path: repo_dir,
        indexed_at: None,
        last_accessed_at: Some(Utc::now()),
        symbol_count: 0,
    };

    // Persist metadata to the global SQLite DB.
    let store = CacheManager::global_store()?;
    store.upsert_repo(&meta).map_err(RepoError::Storage)?;

    info!("Clone complete for repo_id={}", repo_id);
    Ok(meta)
}

/// Perform the actual libgit2 shallow clone (depth=1).
fn do_shallow_clone(url: &str, branch: Option<&str>, into: &Path) -> Result<(), git2::Error> {
    let mut fetch_opts = FetchOptions::new();
    // Shallow clone: fetch only the most recent commit.
    fetch_opts.depth(1);

    let mut builder = git2::build::RepoBuilder::new();
    builder.fetch_options(fetch_opts);

    if let Some(b) = branch {
        builder.branch(b);
    }

    if let Err(e) = builder.clone(url, into) {
        // Fallback to git CLI if git2 fails (e.g. TLS stream error)
        tracing::warn!("git2 clone failed ({}). Falling back to git CLI.", e);
        let mut cmd = std::process::Command::new("git");
        cmd.args(["clone", "--depth", "1"]);
        if let Some(b) = branch {
            cmd.args(["-b", b]);
        }
        let status = cmd
            .arg(url)
            .arg(into.to_str().unwrap())
            .status()
            .map_err(|io_err| {
                git2::Error::from_str(&format!("git CLI fallback failed: {}", io_err))
            })?;

        if !status.success() {
            return Err(git2::Error::from_str("git CLI clone failed"));
        }
    }

    Ok(())
}
