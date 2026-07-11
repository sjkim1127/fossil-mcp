//! JavaScript/TypeScript dependency indexer.
//!
//! Parses package.json and indexes source from node_modules.

use std::path::{Path, PathBuf};

use fossil_core::{storage::GlobalStore, types::SymbolSource};
use serde::Deserialize;
use tracing::{info, warn};

use super::DepIndexResult;
use crate::error::IndexError;
use crate::parser::ParserRegistry;
use crate::symbol::index_directory;

#[derive(Debug, Deserialize)]
struct PackageJson {
    #[serde(default)]
    dependencies: std::collections::HashMap<String, String>,
    #[serde(rename = "devDependencies", default)]
    dev_dependencies: std::collections::HashMap<String, String>,
}

/// Index JS/TS dependencies from `node_modules` in `workspace_path`.
pub fn index_js_deps(
    workspace_path: &Path,
    store: &GlobalStore,
    registry: &ParserRegistry,
    include_dev: bool,
) -> Result<Vec<DepIndexResult>, IndexError> {
    let pkg_path = workspace_path.join("package.json");
    if !pkg_path.exists() {
        return Err(IndexError::ParseFailed {
            file: pkg_path.display().to_string(),
            message: "package.json not found.".to_string(),
        });
    }

    let content = std::fs::read_to_string(&pkg_path).map_err(IndexError::Io)?;
    let pkg: PackageJson = serde_json::from_str(&content).map_err(|e| IndexError::ParseFailed {
        file: pkg_path.display().to_string(),
        message: e.to_string(),
    })?;

    let node_modules = workspace_path.join("node_modules");
    if !node_modules.exists() {
        return Err(IndexError::ParseFailed {
            file: node_modules.display().to_string(),
            message: "node_modules not found. Run `npm install` first.".to_string(),
        });
    }

    let mut all_deps: Vec<(String, String)> = pkg.dependencies.into_iter().collect();
    if include_dev {
        all_deps.extend(pkg.dev_dependencies.into_iter());
    }

    info!("Found {} JS/TS dependencies to index", all_deps.len());

    let mut results = Vec::new();
    for (name, version_range) in all_deps {
        // Resolve actual installed version from node_modules/<name>/package.json
        let pkg_dir = node_modules.join(&name);
        if !pkg_dir.is_dir() {
            warn!("Package '{}' not found in node_modules", name);
            continue;
        }

        let actual_version = read_installed_version(&pkg_dir).unwrap_or(version_range);

        match store.is_package_indexed(&name, &actual_version, "typescript") {
            Ok(Some(n)) => {
                results.push(DepIndexResult {
                    package_name: name,
                    package_version: actual_version,
                    language: "typescript".to_string(),
                    source_path: String::new(),
                    symbol_count: n as usize,
                    was_cached: true,
                });
                continue;
            }
            Ok(None) => {}
            Err(e) => warn!("Cache check error: {}", e),
        }

        info!(
            "Indexing JS/TS package {} {} from {}",
            name,
            actual_version,
            pkg_dir.display()
        );
        let repo_id = format!("npm:{}:{}", name, actual_version);

        let (mut symbols, edges) =
            index_directory(&pkg_dir, &repo_id, registry).map_err(|e| IndexError::ParseFailed {
                file: pkg_dir.display().to_string(),
                message: e.to_string(),
            })?;

        for sym in &mut symbols {
            sym.source = SymbolSource::ExternalDep;
            sym.package_name = Some(name.clone());
            sym.package_version = Some(actual_version.clone());
        }

        let sym_count = symbols.len();
        store
            .insert_symbols(&symbols)
            .map_err(|e| IndexError::ParseFailed {
                file: String::new(),
                message: format!("DB error: {}", e),
            })?;
        if !edges.is_empty() {
            let _ = store.insert_call_edges(&edges);
        }

        let _ = store.mark_package_indexed(
            &name,
            &actual_version,
            "typescript",
            &pkg_dir.to_string_lossy(),
            sym_count as i64,
        );

        results.push(DepIndexResult {
            package_name: name,
            package_version: actual_version,
            language: "typescript".to_string(),
            source_path: pkg_dir.to_string_lossy().to_string(),
            symbol_count: sym_count,
            was_cached: false,
        });
    }

    Ok(results)
}

fn read_installed_version(pkg_dir: &Path) -> Option<String> {
    let pkg_json = pkg_dir.join("package.json");
    let content = std::fs::read_to_string(pkg_json).ok()?;
    let val: serde_json::Value = serde_json::from_str(&content).ok()?;
    val["version"].as_str().map(|s| s.to_string())
}
