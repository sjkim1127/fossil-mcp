//! C/C++ dependency indexer.
//!
//! Resolves dependencies from vcpkg.json, conanfile.txt, or CMakeLists.txt
//! and indexes headers from system paths and package manager install dirs.

use std::path::{Path, PathBuf};

use fossil_core::{storage::GlobalStore, types::SymbolSource};
use tracing::{info, warn};

use super::DepIndexResult;
use crate::error::IndexError;
use crate::parser::ParserRegistry;
use crate::symbol::index_directory;

/// Index C/C++ dependencies from `workspace_path`.
pub fn index_cpp_deps(
    workspace_path: &Path,
    store: &GlobalStore,
    registry: &ParserRegistry,
) -> Result<Vec<DepIndexResult>, IndexError> {
    let packages = discover_cpp_packages(workspace_path)?;

    if packages.is_empty() {
        return Err(IndexError::ParseFailed {
            file: workspace_path.display().to_string(),
            message: "No vcpkg.json or conanfile.txt found, and no vcpkg_installed dir detected."
                .to_string(),
        });
    }

    info!("Found {} C++ packages to index", packages.len());
    let mut results = Vec::new();

    for (name, version, source_path) in packages {
        match store.is_package_indexed(&name, &version, "cpp") {
            Ok(Some(n)) => {
                results.push(DepIndexResult {
                    package_name: name,
                    package_version: version,
                    language: "cpp".to_string(),
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
            "Indexing C++ package {} {} from {}",
            name,
            version,
            source_path.display()
        );

        let repo_id = format!("vcpkg:{}:{}", name, version);
        let (mut symbols, edges) =
            index_directory(&source_path, &repo_id, registry).map_err(|e| {
                IndexError::ParseFailed {
                    file: source_path.display().to_string(),
                    message: e.to_string(),
                }
            })?;

        for sym in &mut symbols {
            sym.source = SymbolSource::ExternalDep;
            sym.package_name = Some(name.clone());
            sym.package_version = Some(version.clone());
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
            &version,
            "cpp",
            &source_path.to_string_lossy(),
            sym_count as i64,
        );

        results.push(DepIndexResult {
            package_name: name,
            package_version: version,
            language: "cpp".to_string(),
            source_path: source_path.to_string_lossy().to_string(),
            symbol_count: sym_count,
            was_cached: false,
        });
    }

    Ok(results)
}

// ── Discovery ────────────────────────────────────────────────────────────────

/// Returns (name, version, source_path) for each discovered C++ dependency.
fn discover_cpp_packages(workspace: &Path) -> Result<Vec<(String, String, PathBuf)>, IndexError> {
    let mut packages = Vec::new();

    // Strategy 1: vcpkg_installed directory (created after `vcpkg install`)
    let vcpkg_installed = workspace.join("vcpkg_installed");
    if vcpkg_installed.is_dir() {
        packages.extend(scan_vcpkg_installed(&vcpkg_installed));
    }

    // Strategy 2: vcpkg.json manifest
    let vcpkg_json = workspace.join("vcpkg.json");
    if vcpkg_json.exists() && packages.is_empty() {
        warn!("vcpkg.json found but vcpkg_installed/ not present. Run `vcpkg install` first.");
    }

    // Strategy 3: conanfile.txt
    let conanfile = workspace.join("conanfile.txt");
    if conanfile.exists() {
        packages.extend(scan_conan_packages(workspace));
    }

    Ok(packages)
}

/// Scan vcpkg_installed/<triplet>/include/ for headers.
fn scan_vcpkg_installed(vcpkg_installed: &Path) -> Vec<(String, String, PathBuf)> {
    let mut result = Vec::new();
    if let Ok(triplets) = std::fs::read_dir(vcpkg_installed) {
        for triplet in triplets.flatten() {
            let include_dir = triplet.path().join("include");
            if !include_dir.is_dir() {
                continue;
            }
            // Each top-level directory inside include is a package
            if let Ok(packages) = std::fs::read_dir(&include_dir) {
                for pkg in packages.flatten() {
                    if pkg.path().is_dir() {
                        let name = pkg.file_name().to_string_lossy().to_string();
                        result.push((name.clone(), "vcpkg".to_string(), pkg.path()));
                    }
                }
            }
        }
    }
    result
}

/// Scan conan package cache directory.
fn scan_conan_packages(workspace: &Path) -> Vec<(String, String, PathBuf)> {
    let mut result = Vec::new();
    // Conan 2.x: ~/.conan2/p/<name>/
    let conan_home = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".conan2")
        .join("p");

    if conan_home.is_dir() {
        if let Ok(entries) = std::fs::read_dir(&conan_home) {
            for entry in entries.flatten() {
                let include = entry.path().join("p").join("include");
                if include.is_dir() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    result.push((name, "conan".to_string(), include));
                }
            }
        }
    }

    result
}
