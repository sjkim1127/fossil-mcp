//! Rust dependency indexer.
//!
//! Parses Cargo.toml + Cargo.lock to discover all transitive dependencies,
//! then indexes their source code from ~/.cargo/registry/src/.
//!
//! Cache strategy: if `indexed_packages` already has (name, version, "rust"),
//! we skip re-indexing.

use std::path::{Path, PathBuf};

use fossil_core::{
    storage::GlobalStore,
    types::{CallEdge, Symbol, SymbolSource},
};
use serde::Deserialize;
use tracing::{debug, info, warn};

use super::DepIndexResult;
use crate::error::IndexError;
use crate::parser::ParserRegistry;
use crate::symbol::index_directory;

// ── Cargo manifest types ──────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct CargoLock {
    package: Vec<LockPackage>,
}

#[derive(Debug, Deserialize)]
struct LockPackage {
    name: String,
    version: String,
    #[serde(default)]
    source: Option<String>,
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Index all transitive Rust dependencies for the project at `workspace_path`.
///
/// Returns a list of `DepIndexResult` describing what was indexed or skipped.
pub fn index_rust_deps(
    workspace_path: &Path,
    store: &GlobalStore,
    registry: &ParserRegistry,
) -> Result<Vec<DepIndexResult>, IndexError> {
    let lock_path = workspace_path.join("Cargo.lock");
    if !lock_path.exists() {
        return Err(IndexError::ParseFailed {
            file: lock_path.display().to_string(),
            message: "Cargo.lock not found. Run `cargo build` first to generate it.".to_string(),
        });
    }

    let lock_content = std::fs::read_to_string(&lock_path).map_err(IndexError::Io)?;
    let lock: CargoLock = toml::from_str(&lock_content).map_err(|e| IndexError::ParseFailed {
        file: lock_path.display().to_string(),
        message: e.to_string(),
    })?;

    info!(
        "Found {} packages in Cargo.lock ({})",
        lock.package.len(),
        lock_path.display()
    );

    let cargo_registry = cargo_registry_root();
    let mut results = Vec::new();

    for pkg in &lock.package {
        // Skip packages without a registry source (path deps, workspace members)
        if pkg.source.is_none() {
            debug!("Skipping local/path dep: {} {}", pkg.name, pkg.version);
            continue;
        }

        // Check cache first
        match store.is_package_indexed(&pkg.name, &pkg.version, "rust") {
            Ok(Some(sym_count)) => {
                debug!(
                    "Cache hit: {} {} ({} symbols)",
                    pkg.name, pkg.version, sym_count
                );
                results.push(DepIndexResult {
                    package_name: pkg.name.clone(),
                    package_version: pkg.version.clone(),
                    language: "rust".to_string(),
                    source_path: String::new(),
                    symbol_count: sym_count as usize,
                    was_cached: true,
                });
                continue;
            }
            Ok(None) => {} // Not cached; proceed to index
            Err(e) => warn!("Cache check failed for {}: {}", pkg.name, e),
        }

        // Locate source in ~/.cargo/registry/src/**/<name>-<version>/
        let source_path = match find_cargo_source(&cargo_registry, &pkg.name, &pkg.version) {
            Some(p) => p,
            None => {
                warn!(
                    "Source not found in cargo registry for {} {}. Run `cargo fetch` first.",
                    pkg.name, pkg.version
                );
                continue;
            }
        };

        info!(
            "Indexing {} {} from {}",
            pkg.name,
            pkg.version,
            source_path.display()
        );

        // Use a stable synthetic repo_id for this package
        let repo_id = format!("crate:{}:{}", pkg.name, pkg.version);

        let (mut symbols, edges) =
            index_directory(&source_path, &repo_id, registry).map_err(|e| {
                IndexError::ParseFailed {
                    file: source_path.display().to_string(),
                    message: e.to_string(),
                }
            })?;

        // Tag all symbols as external_dep
        for sym in &mut symbols {
            sym.source = SymbolSource::ExternalDep;
            sym.package_name = Some(pkg.name.clone());
            sym.package_version = Some(pkg.version.clone());
        }

        let sym_count = symbols.len();

        // Persist to DB
        store
            .insert_symbols(&symbols)
            .map_err(|e| IndexError::ParseFailed {
                file: String::new(),
                message: format!("DB insert failed: {}", e),
            })?;

        if !edges.is_empty() {
            store
                .insert_call_edges(&edges)
                .map_err(|e| IndexError::ParseFailed {
                    file: String::new(),
                    message: format!("DB edge insert failed: {}", e),
                })?;
        }

        // Mark as cached
        let _ = store.mark_package_indexed(
            &pkg.name,
            &pkg.version,
            "rust",
            &source_path.to_string_lossy(),
            sym_count as i64,
        );

        results.push(DepIndexResult {
            package_name: pkg.name.clone(),
            package_version: pkg.version.clone(),
            language: "rust".to_string(),
            source_path: source_path.to_string_lossy().to_string(),
            symbol_count: sym_count,
            was_cached: false,
        });
    }

    Ok(results)
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Returns the root of the Cargo registry source cache.
/// Typically ~/.cargo/registry/src/
fn cargo_registry_root() -> PathBuf {
    let cargo_home = std::env::var("CARGO_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".cargo")
        });
    cargo_home.join("registry").join("src")
}

/// Search ~/.cargo/registry/src/**/<name>-<version> for the source directory.
fn find_cargo_source(registry_root: &Path, name: &str, version: &str) -> Option<PathBuf> {
    let dir_name = format!("{}-{}", name, version);

    // registry_root contains index-named subdirs (e.g. index.crates.io-...)
    if let Ok(entries) = std::fs::read_dir(registry_root) {
        for entry in entries.flatten() {
            let candidate = entry.path().join(&dir_name);
            if candidate.is_dir() {
                return Some(candidate);
            }
        }
    }
    None
}
