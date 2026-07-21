//! Real-time workspace file watcher and debounced incremental indexer.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fossil_core::storage::GlobalStore;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use crate::parser::ParserRegistry;
use crate::symbol::index_single_file;

/// Manages real-time file system monitoring for a single workspace repository.
pub struct WorkspaceWatcher {
    repo_id: String,
    workspace_path: PathBuf,
    _watcher: RecommendedWatcher,
    task_handle: JoinHandle<()>,
}

impl WorkspaceWatcher {
    /// Starts watching `workspace_path` for `repo_id`.
    /// Changes are collected and processed in batches every `debounce_duration` (default 1s).
    pub fn start(
        workspace_path: PathBuf,
        repo_id: String,
        store: GlobalStore,
        registry: ParserRegistry,
        debounce_duration: Duration,
    ) -> Result<Self, String> {
        let pending_changes = Arc::new(Mutex::new(HashSet::<PathBuf>::new()));
        let pending_changes_clone = Arc::clone(&pending_changes);

        // Build notify watcher
        let mut watcher =
            notify::recommended_watcher(move |res: Result<Event, notify::Error>| match res {
                Ok(event) => {
                    if is_relevant_event(&event) {
                        let mut set = pending_changes_clone.lock().unwrap();
                        for path in event.paths {
                            if should_index_file(&path) {
                                set.insert(path);
                            }
                        }
                    }
                }
                Err(e) => warn!("File watcher error: {:?}", e),
            })
            .map_err(|e| format!("Failed to create notify watcher: {}", e))?;

        watcher
            .watch(&workspace_path, RecursiveMode::Recursive)
            .map_err(|e| {
                format!(
                    "Failed to watch directory {}: {}",
                    workspace_path.display(),
                    e
                )
            })?;

        let repo_id_task = repo_id.clone();
        let workspace_path_task = workspace_path.clone();
        let pending_task = Arc::clone(&pending_changes);

        // Spawn background debouncer task
        let task_handle = tokio::spawn(async move {
            let mut interval = tokio::time::interval(debounce_duration);
            loop {
                interval.tick().await;

                let paths_to_process: Vec<PathBuf> = {
                    let mut set = pending_task.lock().unwrap();
                    set.drain().collect()
                };

                if paths_to_process.is_empty() {
                    continue;
                }

                debug!(
                    "Processing {} debounced file changes for repo_id={}",
                    paths_to_process.len(),
                    repo_id_task
                );

                for path in paths_to_process {
                    if path.exists() && path.is_file() {
                        // File created or modified
                        match index_single_file(
                            &path,
                            &workspace_path_task,
                            &repo_id_task,
                            &registry,
                        ) {
                            Ok((symbols, edges)) => {
                                match store.update_file_symbols(
                                    &repo_id_task,
                                    &path.to_string_lossy(),
                                    &symbols,
                                    &edges,
                                ) {
                                    Ok(inserted) => {
                                        info!(
                                            "Updated index for {}: {} symbols",
                                            path.display(),
                                            inserted.len()
                                        );
                                    }
                                    Err(e) => {
                                        warn!("Failed DB update for {}: {}", path.display(), e)
                                    }
                                }
                            }
                            Err(e) => {
                                debug!("Failed parsing single file {}: {}", path.display(), e)
                            }
                        }
                    } else {
                        // File removed
                        let rel_path = path
                            .strip_prefix(&workspace_path_task)
                            .unwrap_or(&path)
                            .to_string_lossy()
                            .to_string();
                        match store.remove_file_symbols(&repo_id_task, &rel_path) {
                            Ok(count) => {
                                info!("Removed {} symbols for deleted file {}", count, rel_path);
                            }
                            Err(e) => warn!("Failed DB removal for {}: {}", rel_path, e),
                        }
                    }
                }
            }
        });

        info!(
            "Started WorkspaceWatcher for repo_id={} at {}",
            repo_id,
            workspace_path.display()
        );

        Ok(Self {
            repo_id,
            workspace_path,
            _watcher: watcher,
            task_handle,
        })
    }

    /// Stops the watcher task.
    pub fn stop(self) {
        self.task_handle.abort();
        info!(
            "Stopped WorkspaceWatcher for repo_id={} at {}",
            self.repo_id,
            self.workspace_path.display()
        );
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn is_relevant_event(event: &Event) -> bool {
    matches!(
        event.kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
    )
}

fn should_index_file(path: &Path) -> bool {
    let path_str = path.to_string_lossy();
    if path_str.contains("/.git/")
        || path_str.contains("/node_modules/")
        || path_str.contains("/target/")
        || path_str.contains("/__pycache__/")
        || path_str.contains("/.venv/")
        || path_str.contains("/.cargo/")
        || path_str.contains("/dist/")
        || path_str.contains("/build/")
    {
        return false;
    }

    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    matches!(
        ext,
        "rs" | "py" | "js" | "ts" | "jsx" | "tsx" | "cpp" | "c" | "h" | "hpp" | "cc" | "cxx"
    )
}
