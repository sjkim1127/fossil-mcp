//! Python dependency indexer.
//!
//! Parses requirements.txt / pyproject.toml to discover dependencies,
//! then indexes their source code from the local site-packages directory.

use std::path::{Path, PathBuf};

use fossil_core::{storage::GlobalStore, types::SymbolSource};
use tracing::{debug, info, warn};

use super::DepIndexResult;
use crate::error::IndexError;
use crate::parser::ParserRegistry;
use crate::symbol::index_directory;

/// Index Python dependencies found in `workspace_path`.
pub fn index_python_deps(
    workspace_path: &Path,
    store: &GlobalStore,
    registry: &ParserRegistry,
) -> Result<Vec<DepIndexResult>, IndexError> {
    let packages = discover_python_packages(workspace_path)?;
    if packages.is_empty() {
        return Err(IndexError::ParseFailed {
            file: workspace_path.display().to_string(),
            message: "No requirements.txt or pyproject.toml found.".to_string(),
        });
    }

    info!("Discovered {} Python packages to index", packages.len());

    let site_packages = find_site_packages();
    let mut results = Vec::new();

    for (pkg_name, pkg_version) in packages {
        // Check cache
        match store.is_package_indexed(&pkg_name, &pkg_version, "python") {
            Ok(Some(n)) => {
                results.push(DepIndexResult {
                    package_name: pkg_name,
                    package_version: pkg_version,
                    language: "python".to_string(),
                    source_path: String::new(),
                    symbol_count: n as usize,
                    was_cached: true,
                });
                continue;
            }
            Ok(None) => {}
            Err(e) => warn!("Cache check error: {}", e),
        }

        // Try to find the package in site-packages
        let source_path = match find_python_source(&site_packages, &pkg_name) {
            Some(p) => p,
            None => {
                warn!(
                    "Python package '{}' not found in site-packages. Run `pip install {}` first.",
                    pkg_name, pkg_name
                );
                continue;
            }
        };

        info!(
            "Indexing Python package {} {} from {}",
            pkg_name,
            pkg_version,
            source_path.display()
        );

        let repo_id = format!("pypi:{}:{}", pkg_name, pkg_version);
        let (mut symbols, edges) =
            index_directory(&source_path, &repo_id, registry).map_err(|e| {
                IndexError::ParseFailed {
                    file: source_path.display().to_string(),
                    message: e.to_string(),
                }
            })?;

        for sym in &mut symbols {
            sym.source = SymbolSource::ExternalDep;
            sym.package_name = Some(pkg_name.clone());
            sym.package_version = Some(pkg_version.clone());
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
            &pkg_name,
            &pkg_version,
            "python",
            &source_path.to_string_lossy(),
            sym_count as i64,
        );

        results.push(DepIndexResult {
            package_name: pkg_name,
            package_version: pkg_version,
            language: "python".to_string(),
            source_path: source_path.to_string_lossy().to_string(),
            symbol_count: sym_count,
            was_cached: false,
        });
    }

    Ok(results)
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Parse requirements.txt and pyproject.toml to get (name, version) pairs.
/// Version may be "unknown" if not pinned.
fn discover_python_packages(workspace: &Path) -> Result<Vec<(String, String)>, IndexError> {
    let mut packages = Vec::new();

    // requirements.txt
    let req_path = workspace.join("requirements.txt");
    if req_path.exists() {
        let content = std::fs::read_to_string(&req_path).map_err(IndexError::Io)?;
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with('-') {
                continue;
            }
            // Handle: package==1.0.0, package>=1.0, package~=1.0, package
            if let Some((name, version)) = parse_req_line(line) {
                packages.push((name, version));
            }
        }
    }

    // pyproject.toml (basic support)
    let pyproject_path = workspace.join("pyproject.toml");
    if pyproject_path.exists() {
        let content = std::fs::read_to_string(&pyproject_path).map_err(IndexError::Io)?;
        packages.extend(parse_pyproject_deps(&content));
    }

    // Deduplicate
    packages.sort();
    packages.dedup_by_key(|(name, _)| name.clone());

    Ok(packages)
}

fn parse_req_line(line: &str) -> Option<(String, String)> {
    // Remove extras like package[extra]==version
    let base = line.split('[').next().unwrap_or(line);
    let separators = ['=', '>', '<', '~', '!'];
    if let Some(pos) = base.find(separators) {
        let name = base[..pos].trim().to_lowercase().replace('-', "_");
        let version = line[pos..]
            .trim_start_matches(['=', '>', '<', '~', '!'])
            .trim()
            .to_string();
        Some((
            name,
            if version.is_empty() {
                "unknown".to_string()
            } else {
                version
            },
        ))
    } else {
        Some((
            base.trim().to_lowercase().replace('-', "_"),
            "unknown".to_string(),
        ))
    }
}

fn parse_pyproject_deps(content: &str) -> Vec<(String, String)> {
    let mut deps = Vec::new();
    let mut in_deps = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed == "[project]" || trimmed.contains("dependencies") {
            in_deps = true;
            continue;
        }
        if trimmed.starts_with('[') {
            in_deps = false;
        }
        if in_deps && (trimmed.starts_with('"') || trimmed.starts_with('\'')) {
            let clean = trimmed.trim_matches(['"', '\'', ',']);
            if let Some((name, version)) = parse_req_line(clean) {
                deps.push((name, version));
            }
        }
    }
    deps
}

/// Returns all candidate site-packages paths on this system.
fn find_site_packages() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    // Try `python3 -c "import site; print('\n'.join(site.getsitepackages()))"``
    if let Ok(out) = std::process::Command::new("python3")
        .args([
            "-c",
            "import site; [print(p) for p in site.getsitepackages()]",
        ])
        .output()
    {
        if out.status.success() {
            for line in String::from_utf8_lossy(&out.stdout).lines() {
                let p = PathBuf::from(line.trim());
                if p.is_dir() {
                    paths.push(p);
                }
            }
        }
    }

    // Also try user site
    if let Ok(out) = std::process::Command::new("python3")
        .args(["-c", "import site; print(site.getusersitepackages())"])
        .output()
    {
        let p = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim().to_string());
        if p.is_dir() && !paths.contains(&p) {
            paths.push(p);
        }
    }

    paths
}

/// Find a Python package directory in site-packages.
/// Normalises name: hyphens → underscores, case-insensitive.
fn find_python_source(site_packages: &[PathBuf], pkg_name: &str) -> Option<PathBuf> {
    let normalized = pkg_name.to_lowercase().replace('-', "_");
    for sp in site_packages {
        if let Ok(entries) = std::fs::read_dir(sp) {
            for entry in entries.flatten() {
                let name = entry
                    .file_name()
                    .to_string_lossy()
                    .to_lowercase()
                    .replace('-', "_");
                if name == normalized && entry.path().is_dir() {
                    return Some(entry.path());
                }
            }
        }
    }
    None
}
