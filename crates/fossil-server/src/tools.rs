use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use chrono::Utc;
use rmcp::{
    ErrorData as McpError, ServerHandler, handler::server::wrapper::Parameters, schemars, tool,
    tool_handler, tool_router,
};
use serde::Deserialize;
use serde_json::json;
use tracing::{debug, info};

use fossil_indexer::{
    WorkspaceWatcher,
    deps::{index_cpp_deps, index_js_deps, index_python_deps, index_rust_deps},
    index_directory, index_single_file,
    languages::default_registry,
    migration, parse_scip_index,
};
use fossil_repo::{cache::CacheManager, clone::CloneOptions, history};
use fossil_search::{FuzzySearcher, semantic::SemanticSearcher, traits::Searcher};

// ── Input types ───────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CloneReferenceInput {
    #[schemars(description = "Public git repository URL to clone")]
    pub repo_url: String,
    #[schemars(description = "Optional human-friendly alias for this repo")]
    pub alias: Option<String>,
    #[schemars(description = "Branch or tag name to check out (default: repo default branch)")]
    pub branch: Option<String>,
    #[schemars(description = "If true, delete existing cache and re-clone")]
    pub refresh: Option<bool>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct AnalyzeMigrationInput {
    #[schemars(description = "Repository ID containing the migration")]
    pub repo_id: String,
    #[schemars(description = "The starting git revision (e.g. 'v16.0.0' or commit hash)")]
    pub start_revision: String,
    #[schemars(description = "The ending git revision (e.g. 'v18.0.0' or commit hash)")]
    pub end_revision: String,
    #[schemars(description = "File glob pattern to analyze (e.g. '*.ts' or '*.rs')")]
    pub file_pattern: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ScanVulnerabilitiesInput {
    #[schemars(description = "Absolute path to the local workspace directory to scan")]
    pub workspace_path: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct IndexRepoInput {
    #[schemars(description = "Repository ID returned by clone_reference")]
    pub repo_id: String,
    #[schemars(
        description = "Language filter e.g. [\"rust\", \"python\"]. Empty means all supported languages."
    )]
    pub languages: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct LocateImplementationInput {
    #[schemars(
        description = "Repository ID returned by clone_reference. If omitted, searches across all indexed repositories."
    )]
    pub repo_id: Option<String>,
    #[schemars(description = "Natural language or keyword query (e.g. 'OAuth token refresh')")]
    pub query: String,
    #[schemars(description = "Maximum number of results to return (default: 5)")]
    pub top_k: Option<u32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GetSymbolSourceInput {
    #[schemars(description = "Repository ID")]
    pub repo_id: String,
    #[schemars(description = "File path relative to repo root")]
    pub file_path: String,
    #[schemars(description = "First line of the symbol (1-indexed)")]
    pub line_start: u32,
    #[schemars(description = "Last line of the symbol (1-indexed, inclusive)")]
    pub line_end: u32,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct AnalyzeFeatureInput {
    #[schemars(description = "Public git repository URL to clone and analyze")]
    pub repo_url: String,
    #[schemars(
        description = "Natural language or keyword query describing the feature (e.g. 'OAuth token refresh')"
    )]
    pub query: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListIndexedReposInput {}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct IndexDepsInput {
    #[schemars(description = "Absolute path to the workspace directory to scan for dependencies")]
    pub workspace_path: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct IndexJsDepsInput {
    #[schemars(description = "Absolute path to the workspace directory to scan for dependencies")]
    pub workspace_path: String,
    #[schemars(description = "Whether to include devDependencies (default: false)")]
    pub include_dev: Option<bool>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct UpdateFileIndexInput {
    #[schemars(description = "Absolute path to workspace directory")]
    pub workspace_path: String,
    #[schemars(description = "Repository ID")]
    pub repo_id: String,
    #[schemars(description = "File path (relative to workspace or absolute)")]
    pub file_path: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RemoveFileIndexInput {
    #[schemars(description = "Absolute path to workspace directory")]
    pub workspace_path: String,
    #[schemars(description = "Repository ID")]
    pub repo_id: String,
    #[schemars(description = "File path (relative to workspace or absolute)")]
    pub file_path: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct WatchWorkspaceInput {
    #[schemars(description = "Absolute path to workspace directory to watch")]
    pub workspace_path: String,
    #[schemars(description = "Repository ID")]
    pub repo_id: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct UnwatchWorkspaceInput {
    #[schemars(description = "Absolute path to workspace directory to stop watching")]
    pub workspace_path: String,
}

// ── Server ────────────────────────────────────────────────────────────────────

/// The fossil-mcp MCP server.
#[derive(Clone)]
pub struct FossilServer {
    watchers: Arc<Mutex<HashMap<String, WorkspaceWatcher>>>,
}

impl FossilServer {
    pub fn new() -> Self {
        Self {
            watchers: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

#[tool_router]
impl FossilServer {
    // ── clone_reference ──────────────────────────────────────────────────────

    #[tool(
        description = "Clone a public git repository into the local fossil-mcp cache. Reuses existing clones unless refresh=true. Returns repo_id needed for subsequent calls."
    )]
    async fn clone_reference(
        &self,
        Parameters(input): Parameters<CloneReferenceInput>,
    ) -> Result<String, McpError> {
        info!("clone_reference: url={}", input.repo_url);

        let url = input.repo_url.clone();
        let alias = input.alias.clone();
        let branch = input.branch.clone();
        let refresh = input.refresh.unwrap_or(false);

        let meta = tokio::task::spawn_blocking(move || {
            fossil_repo::clone::clone_repo(CloneOptions {
                url: &url,
                alias,
                branch,
                refresh,
            })
        })
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        // Enforce a 5GB cache limit after cloning a new repository
        if let Ok(store) = CacheManager::global_store() {
            let _ = store.mark_accessed(&meta.repo_id);
            let limit_5gb = 5 * 1024 * 1024 * 1024;
            if let Err(e) = CacheManager::enforce_capacity(&store, limit_5gb) {
                tracing::warn!("Failed to enforce cache capacity: {}", e);
            }
        }

        Ok(json!({
            "repo_id": meta.repo_id,
            "path": meta.path.to_string_lossy(),
            "indexed": meta.indexed_at.is_some(),
        })
        .to_string())
    }

    // ── index_repo ───────────────────────────────────────────────────────────

    #[tool(
        description = "Parse and index all source files in a cloned repository. Extracts symbols (functions, structs, classes, etc.) and builds a 1-hop call graph. Must be run after clone_reference."
    )]
    async fn index_repo(
        &self,
        Parameters(input): Parameters<IndexRepoInput>,
    ) -> Result<String, McpError> {
        info!("index_repo: repo_id={}", input.repo_id);

        let repo_id = input.repo_id.clone();
        let languages = input.languages.clone();

        let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, String> {
            let repo_dir = CacheManager::repo_dir(&repo_id);
            if !repo_dir.exists() {
                return Err(format!(
                    "Repository '{}' not found in cache. Run clone_reference first.",
                    repo_id
                ));
            }

            let registry = default_registry();
            let start = Instant::now();

            let scip_path = repo_dir.join("index.scip");

            // Try to auto-generate SCIP if not present
            if !scip_path.exists() && repo_dir.join("Cargo.toml").exists() {
                info!(
                    "Cargo.toml found. Attempting to generate SCIP index via 'rust-analyzer scip .'"
                );
                let _ = std::process::Command::new("rust-analyzer")
                    .arg("scip")
                    .arg(".")
                    .current_dir(&repo_dir)
                    .output();
            }

            let (mut symbols, call_edges) = if scip_path.exists() {
                info!("Found index.scip, using SCIP indexer for high precision.");
                parse_scip_index(&scip_path, &repo_id).map_err(|e| e.to_string())?
            } else {
                info!("No index.scip found, falling back to tree-sitter.");
                index_directory(&repo_dir, &repo_id, &registry).map_err(|e| e.to_string())?
            };

            // Apply language filter if requested.
            if let Some(ref langs) = languages
                && !langs.is_empty()
            {
                symbols.retain(|s| langs.iter().any(|l| l.eq_ignore_ascii_case(&s.language)));
            }

            let duration_ms = start.elapsed().as_millis() as u64;
            let files_indexed = symbols
                .iter()
                .map(|s| s.file_path.clone())
                .collect::<std::collections::HashSet<_>>()
                .len();
            let symbol_count = symbols.len() as u64;

            // Persist to SQLite.
            let store = CacheManager::global_store().map_err(|e| e.to_string())?;
            let _ = store.mark_accessed(&repo_id); // Update LRU tracking
            store.clear_symbols(&repo_id).map_err(|e| e.to_string())?;
            store.insert_symbols(&symbols).map_err(|e| e.to_string())?;

            // Retrieve inserted symbols so we have their generated IDs for embedding
            let stored_symbols = store
                .load_symbols(Some(&repo_id))
                .map_err(|e| e.to_string())?;

            // Generate embeddings in chunks of 500 to avoid memory spikes
            let mut embedding_count = 0;
            for chunk in stored_symbols.chunks(500) {
                let texts: Vec<String> = chunk
                    .iter()
                    .map(|s| format!("{} {} {}", s.language, s.kind, s.signature))
                    .collect();

                let embeddings =
                    SemanticSearcher::generate_embeddings(texts).map_err(|e| e.to_string())?;

                let insert_batch: Vec<(i64, Vec<f32>)> = chunk
                    .iter()
                    .zip(embeddings.into_iter())
                    .map(|(s, vec)| (s.id.unwrap_or(0), vec))
                    .collect();

                embedding_count += store
                    .insert_embeddings(&insert_batch)
                    .map_err(|e| e.to_string())?;
            }
            info!("Generated and inserted {} embeddings", embedding_count);

            store
                .insert_call_edges(&call_edges)
                .map_err(|e| e.to_string())?;
            store
                .update_symbol_count(&repo_id, symbol_count, Utc::now())
                .map_err(|e| e.to_string())?;

            Ok(json!({
                "symbol_count": symbol_count,
                "files_indexed": files_indexed,
                "duration_ms": duration_ms,
            }))
        })
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?
        .map_err(|e| McpError::internal_error(e, None))?;

        Ok(result.to_string())
    }

    // ── locate_implementation ─────────────────────────────────────────────────

    #[tool(
        description = "Search for code symbols matching a natural language or keyword query. Returns file paths, line ranges, signatures, and 1-hop related symbols (calls/called_by). Run index_repo first."
    )]
    async fn locate_implementation(
        &self,
        Parameters(input): Parameters<LocateImplementationInput>,
    ) -> Result<String, McpError> {
        info!(
            "locate_implementation: repo_id={:?} query={:?}",
            input.repo_id, input.query
        );

        let top_k = input.top_k.unwrap_or(5) as usize;
        let repo_id = input.repo_id.clone();
        let query = input.query.clone();

        let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, String> {
            let store = CacheManager::global_store().map_err(|e| e.to_string())?;
            if let Some(ref r_id) = repo_id {
                let _ = store.mark_accessed(r_id); // Update LRU tracking
            }
            let symbols = store
                .load_symbols(repo_id.as_deref())
                .map_err(|e| e.to_string())?;

            if symbols.is_empty() {
                return Err(format!(
                    "Repository '{:?}' has no indexed symbols. Run index_repo first.",
                    repo_id
                ));
            }

            // Try Semantic Search first
            let mut search_results = Vec::new();
            if let Ok(mut query_embeds) = SemanticSearcher::generate_embeddings(vec![query.clone()])
                && let Some(query_embed) = query_embeds.pop()
            {
                match store.search_embeddings(&query_embed, top_k, repo_id.as_deref()) {
                    Ok(vec_results) => {
                        for (sym_id, distance) in vec_results {
                            if let Some(sym) = symbols.iter().find(|s| s.id == Some(sym_id)) {
                                search_results.push((sym.clone(), distance));
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!("search_embeddings failed: {:?}", e);
                    }
                }
            }

            // Fallback to fuzzy search if semantic search yields nothing
            if search_results.is_empty() {
                let results = FuzzySearcher.search(&query, &symbols, top_k);
                for res in results {
                    search_results.push((res.symbol, res.score));
                }
            }

            let matches: Vec<serde_json::Value> = search_results
                .iter()
                .map(|(sym, score)| {
                    // Fetch 1-hop related symbols from call edges.
                    let mut related = Vec::new();

                    if let Ok(callees) = store.calls_made_by(&sym.name, repo_id.as_deref()) {
                        for edge in callees {
                            related.push(json!({
                                "repo_id": edge.repo_id,
                                "name": edge.callee,
                                "relation": "calls",
                                "file_path": edge.file_path,
                                "line": edge.line,
                            }));
                        }
                    }
                    if let Ok(callers) = store.callers_of(&sym.name, repo_id.as_deref()) {
                        for edge in callers {
                            related.push(json!({
                                "repo_id": edge.repo_id,
                                "name": edge.caller,
                                "relation": "called_by",
                                "file_path": edge.file_path,
                                "line": edge.line,
                            }));
                        }
                    }

                    json!({
                        "repo_id": sym.repo_id,
                        "symbol_name": sym.name,
                        "kind": sym.kind.to_string(),
                        "file_path": sym.file_path,
                        "line_start": sym.line_start,
                        "line_end": sym.line_end,
                        "signature": sym.signature,
                        "score": score,
                        "related_symbols": related,
                    })
                })
                .collect();

            Ok(json!({ "matches": matches }))
        })
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?
        .map_err(|e| McpError::internal_error(e, None))?;

        Ok(result.to_string())
    }

    // ── get_symbol_source ────────────────────────────────────────────────────

    #[tool(
        description = "Return the raw source code of a specific file line-range inside a repository. Use file_path and line numbers from locate_implementation results."
    )]
    async fn get_symbol_source(
        &self,
        Parameters(input): Parameters<GetSymbolSourceInput>,
    ) -> Result<String, McpError> {
        debug!(
            "get_symbol_source: repo_id={} file={} lines={}..{}",
            input.repo_id, input.file_path, input.line_start, input.line_end
        );

        let repo_id = input.repo_id.clone();

        let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, String> {
            let repo_dir = CacheManager::repo_dir(&repo_id);
            let full_path = repo_dir.join(&input.file_path);

            if !full_path.exists() {
                return Err(format!("File not found: {}", input.file_path));
            }

            let content = std::fs::read_to_string(&full_path)
                .map_err(|e| format!("Failed to read file: {}", e))?;

            // Extract the requested line range (1-indexed, inclusive).
            let start = (input.line_start as usize).saturating_sub(1);
            let end = input.line_end as usize;
            let source: String = content
                .lines()
                .skip(start)
                .take(end.saturating_sub(start))
                .collect::<Vec<_>>()
                .join("\n");

            // Determine language from extension.
            let language = full_path
                .extension()
                .and_then(|e| e.to_str())
                .map(|ext| match ext {
                    "rs" => "rust",
                    "py" => "python",
                    "ts" | "tsx" => "typescript",
                    "js" | "jsx" | "mjs" | "cjs" => "javascript",
                    other => other,
                })
                .unwrap_or("unknown")
                .to_string();

            Ok(json!({
                "source": source,
                "language": language,
            }))
        })
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?
        .map_err(|e| McpError::internal_error(e, None))?;

        Ok(result.to_string())
    }

    // ── list_indexed_repos ───────────────────────────────────────────────────

    #[tool(
        description = "List all repositories present in the fossil-mcp cache, including indexing status."
    )]
    async fn list_indexed_repos(
        &self,
        Parameters(_): Parameters<ListIndexedReposInput>,
    ) -> Result<String, McpError> {
        debug!("list_indexed_repos");

        let result = tokio::task::spawn_blocking(|| -> Result<serde_json::Value, String> {
            let repos = CacheManager::list_all().map_err(|e| e.to_string())?;
            let items: Vec<serde_json::Value> = repos
                .iter()
                .map(|m| {
                    json!({
                        "repo_id": m.repo_id,
                        "alias": m.alias,
                        "url": m.url,
                        "path": m.path.to_string_lossy(),
                        "indexed_at": m.indexed_at.map(|t| t.to_rfc3339()),
                        "symbol_count": m.symbol_count,
                    })
                })
                .collect();
            Ok(json!({ "repos": items }))
        })
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?
        .map_err(|e| McpError::internal_error(e, None))?;

        Ok(result.to_string())
    }

    // ── analyze_feature ──────────────────────────────────────────────────────

    #[tool(
        description = "High-level tool that orchestrates cloning, indexing, and searching in one step. Ideal for one-shot feature analysis."
    )]
    async fn analyze_feature(
        &self,
        Parameters(input): Parameters<AnalyzeFeatureInput>,
    ) -> Result<String, McpError> {
        info!(
            "analyze_feature: url={} query={:?}",
            input.repo_url, input.query
        );

        // 1. Clone
        let clone_result = self
            .clone_reference(Parameters(CloneReferenceInput {
                repo_url: input.repo_url,
                alias: None,
                branch: None,
                refresh: None,
            }))
            .await?;

        let clone_data: serde_json::Value = serde_json::from_str(&clone_result).map_err(|e| {
            McpError::internal_error(format!("Failed to parse clone result: {}", e), None)
        })?;

        let repo_id = clone_data["repo_id"].as_str().unwrap().to_string();
        let indexed = clone_data["indexed"].as_bool().unwrap_or(false);

        // 2. Index (if not already indexed)
        if !indexed {
            self.index_repo(Parameters(IndexRepoInput {
                repo_id: repo_id.clone(),
                languages: None,
            }))
            .await?;
        }

        // 3. Locate
        self.locate_implementation(Parameters(LocateImplementationInput {
            repo_id: Some(repo_id),
            query: input.query,
            top_k: Some(10), // Give a generous top_k for broad feature analysis
        }))
        .await
    }

    // ── analyze_migration ───────────────────────────────────────────────────

    #[tool(
        description = "Analyzes a git history between two revisions to extract structural AST migration patterns."
    )]
    async fn analyze_migration(
        &self,
        Parameters(input): Parameters<AnalyzeMigrationInput>,
    ) -> Result<String, McpError> {
        info!(
            "analyze_migration: repo_id={} revs={}-{}",
            input.repo_id, input.start_revision, input.end_revision
        );

        let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, String> {
            let repo_dir = CacheManager::repo_dir(&input.repo_id);
            if !repo_dir.exists() {
                return Err(format!(
                    "Repository '{}' not found. Run clone_reference first.",
                    input.repo_id
                ));
            }

            let changes = history::get_file_changes(
                &repo_dir,
                &input.start_revision,
                &input.end_revision,
                &input.file_pattern,
            )
            .map_err(|e| e.to_string())?;

            let registry = default_registry();
            let mut patterns = Vec::new();

            for change in changes {
                if let Some(pattern) = migration::extract_structural_diff(
                    &registry,
                    &change.file_path,
                    &change.before_content,
                    &change.after_content,
                    &change.changed_lines_before,
                    &change.changed_lines_after,
                ) {
                    patterns.push(pattern);
                }
            }

            serde_json::to_value(patterns).map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        Ok(result.to_string())
    }

    // ── scan_vulnerabilities ────────────────────────────────────────────────

    #[tool(description = "Scans a local workspace for structural CVE patterns.")]
    async fn scan_vulnerabilities(
        &self,
        Parameters(input): Parameters<ScanVulnerabilitiesInput>,
    ) -> Result<String, McpError> {
        info!("scan_vulnerabilities: workspace={}", input.workspace_path);

        let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, String> {
            let workspace = std::path::Path::new(&input.workspace_path);
            if !workspace.exists() || !workspace.is_dir() {
                return Err(format!(
                    "Workspace '{}' not found or is not a directory.",
                    input.workspace_path
                ));
            }

            let registry = default_registry();
            let mut detected = Vec::new();

            for entry in walkdir::WalkDir::new(workspace)
                .into_iter()
                .filter_map(|e| e.ok())
            {
                if entry.file_type().is_file() {
                    let path = entry.path();
                    // Basic filter to avoid scanning non-code files
                    if let Some(ext) = path.extension().and_then(|e| e.to_str())
                        && registry.for_extension(ext).is_some()
                        && let Ok(content) = std::fs::read_to_string(path)
                        && let Some(path_str) = path.to_str()
                    {
                        let mut results =
                            fossil_indexer::vulnerability::scan_file_for_vulnerabilities(
                                &registry, path_str, &content,
                            );
                        detected.append(&mut results);
                    }
                }
            }

            serde_json::to_value(detected).map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        Ok(result.to_string())
    }

    // ── Dependencies ─────────────────────────────────────────────────────────

    #[tool(
        description = "Analyzes Cargo.lock and indexes all transitive Rust dependencies from local ~/.cargo/registry/src. Skips already indexed versions."
    )]
    async fn index_rust_deps(
        &self,
        Parameters(input): Parameters<IndexDepsInput>,
    ) -> Result<String, McpError> {
        info!("index_rust_deps: workspace={}", input.workspace_path);
        let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, String> {
            let workspace = std::path::Path::new(&input.workspace_path);
            let store = fossil_core::storage::GlobalStore::open(&fossil_core::storage::global_db_path())
                .map_err(|e| e.to_string())?;
            let registry = default_registry();

            let results = index_rust_deps(workspace, &store, &registry)
                .map_err(|e| e.to_string())?;

            let mut indexed = 0;
            let mut cached = 0;
            for r in &results {
                if r.was_cached { cached += 1; } else { indexed += 1; }
            }

            Ok(json!({
                "message": format!("Indexed {} new packages, skipped {} cached packages", indexed, cached),
                "details": results.into_iter().map(|r| json!({
                    "package": r.package_name,
                    "version": r.package_version,
                    "symbols": r.symbol_count,
                    "cached": r.was_cached
                })).collect::<Vec<_>>()
            }))
        })
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        Ok(result.to_string())
    }

    #[tool(
        description = "Analyzes requirements.txt / pyproject.toml and indexes Python dependencies from local site-packages. Skips already indexed versions."
    )]
    async fn index_python_deps(
        &self,
        Parameters(input): Parameters<IndexDepsInput>,
    ) -> Result<String, McpError> {
        info!("index_python_deps: workspace={}", input.workspace_path);
        let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, String> {
            let workspace = std::path::Path::new(&input.workspace_path);
            let store = fossil_core::storage::GlobalStore::open(&fossil_core::storage::global_db_path())
                .map_err(|e| e.to_string())?;
            let registry = default_registry();

            let results = index_python_deps(workspace, &store, &registry)
                .map_err(|e| e.to_string())?;

            let mut indexed = 0;
            let mut cached = 0;
            for r in &results {
                if r.was_cached { cached += 1; } else { indexed += 1; }
            }

            Ok(json!({
                "message": format!("Indexed {} new packages, skipped {} cached packages", indexed, cached),
                "details": results.into_iter().map(|r| json!({
                    "package": r.package_name,
                    "version": r.package_version,
                    "symbols": r.symbol_count,
                    "cached": r.was_cached
                })).collect::<Vec<_>>()
            }))
        })
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        Ok(result.to_string())
    }

    #[tool(
        description = "Analyzes package.json and indexes JS/TS dependencies from local node_modules. Skips already indexed versions."
    )]
    async fn index_js_deps(
        &self,
        Parameters(input): Parameters<IndexJsDepsInput>,
    ) -> Result<String, McpError> {
        info!("index_js_deps: workspace={}", input.workspace_path);
        let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, String> {
            let workspace = std::path::Path::new(&input.workspace_path);
            let store = fossil_core::storage::GlobalStore::open(&fossil_core::storage::global_db_path())
                .map_err(|e| e.to_string())?;
            let registry = default_registry();

            let results = index_js_deps(workspace, &store, &registry, input.include_dev.unwrap_or(false))
                .map_err(|e| e.to_string())?;

            let mut indexed = 0;
            let mut cached = 0;
            for r in &results {
                if r.was_cached { cached += 1; } else { indexed += 1; }
            }

            Ok(json!({
                "message": format!("Indexed {} new packages, skipped {} cached packages", indexed, cached),
                "details": results.into_iter().map(|r| json!({
                    "package": r.package_name,
                    "version": r.package_version,
                    "symbols": r.symbol_count,
                    "cached": r.was_cached
                })).collect::<Vec<_>>()
            }))
        })
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        Ok(result.to_string())
    }

    #[tool(
        description = "Analyzes vcpkg.json, conanfile.txt or CMakeLists and indexes C/C++ dependencies from local installation dirs. Skips already indexed versions."
    )]
    async fn index_cpp_deps(
        &self,
        Parameters(input): Parameters<IndexDepsInput>,
    ) -> Result<String, McpError> {
        info!("index_cpp_deps: workspace={}", input.workspace_path);
        let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, String> {
            let workspace = std::path::Path::new(&input.workspace_path);
            let store = fossil_core::storage::GlobalStore::open(&fossil_core::storage::global_db_path())
                .map_err(|e| e.to_string())?;
            let registry = default_registry();

            let results = index_cpp_deps(workspace, &store, &registry)
                .map_err(|e| e.to_string())?;

            let mut indexed = 0;
            let mut cached = 0;
            for r in &results {
                if r.was_cached { cached += 1; } else { indexed += 1; }
            }

            Ok(json!({
                "message": format!("Indexed {} new packages, skipped {} cached packages", indexed, cached),
                "details": results.into_iter().map(|r| json!({
                    "package": r.package_name,
                    "version": r.package_version,
                    "symbols": r.symbol_count,
                    "cached": r.was_cached
                })).collect::<Vec<_>>()
            }))
        })
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        Ok(result.to_string())
    }

    // ── Incremental Indexing & Watcher ───────────────────────────────────────

    #[tool(
        description = "Re-indexes a single modified/created file and atomically updates DB symbols."
    )]
    async fn update_file_index(
        &self,
        Parameters(input): Parameters<UpdateFileIndexInput>,
    ) -> Result<String, McpError> {
        info!(
            "update_file_index: repo_id={} file={}",
            input.repo_id, input.file_path
        );
        let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, String> {
            let workspace = std::path::Path::new(&input.workspace_path);
            let abs_file_path = if std::path::Path::new(&input.file_path).is_absolute() {
                std::path::PathBuf::from(&input.file_path)
            } else {
                workspace.join(&input.file_path)
            };

            if !abs_file_path.exists() || !abs_file_path.is_file() {
                return Err(format!(
                    "File '{}' not found or not a file",
                    abs_file_path.display()
                ));
            }

            let store =
                fossil_core::storage::GlobalStore::open(&fossil_core::storage::global_db_path())
                    .map_err(|e| e.to_string())?;
            let registry = default_registry();

            let (symbols, edges) =
                index_single_file(&abs_file_path, workspace, &input.repo_id, &registry)
                    .map_err(|e| e.to_string())?;

            let rel_path = abs_file_path
                .strip_prefix(workspace)
                .unwrap_or(&abs_file_path)
                .to_string_lossy()
                .to_string();

            let inserted = store
                .update_file_symbols(&input.repo_id, &rel_path, &symbols, &edges)
                .map_err(|e| e.to_string())?;

            Ok(json!({
                "status": "updated",
                "repo_id": input.repo_id,
                "file_path": rel_path,
                "symbol_count": inserted.len(),
                "call_edge_count": edges.len(),
            }))
        })
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?
        .map_err(|e| McpError::internal_error(e, None))?;

        Ok(result.to_string())
    }

    #[tool(
        description = "Removes all indexed symbols, call edges, and vectors for a deleted file."
    )]
    async fn remove_file_index(
        &self,
        Parameters(input): Parameters<RemoveFileIndexInput>,
    ) -> Result<String, McpError> {
        info!(
            "remove_file_index: repo_id={} file={}",
            input.repo_id, input.file_path
        );
        let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, String> {
            let workspace = std::path::Path::new(&input.workspace_path);
            let abs_file_path = if std::path::Path::new(&input.file_path).is_absolute() {
                std::path::PathBuf::from(&input.file_path)
            } else {
                workspace.join(&input.file_path)
            };

            let rel_path = abs_file_path
                .strip_prefix(workspace)
                .unwrap_or(&abs_file_path)
                .to_string_lossy()
                .to_string();

            let store =
                fossil_core::storage::GlobalStore::open(&fossil_core::storage::global_db_path())
                    .map_err(|e| e.to_string())?;

            let removed_count = store
                .remove_file_symbols(&input.repo_id, &rel_path)
                .map_err(|e| e.to_string())?;

            Ok(json!({
                "status": "removed",
                "repo_id": input.repo_id,
                "file_path": rel_path,
                "removed_symbol_count": removed_count,
            }))
        })
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?
        .map_err(|e| McpError::internal_error(e, None))?;

        Ok(result.to_string())
    }

    #[tool(
        description = "Starts a background filesystem watcher (notify) on a workspace to auto-update modified files."
    )]
    async fn watch_workspace(
        &self,
        Parameters(input): Parameters<WatchWorkspaceInput>,
    ) -> Result<String, McpError> {
        info!(
            "watch_workspace: path={} repo_id={}",
            input.workspace_path, input.repo_id
        );

        let workspace_buf = std::path::PathBuf::from(&input.workspace_path);
        if !workspace_buf.exists() || !workspace_buf.is_dir() {
            return Err(McpError::invalid_params(
                "workspace_path does not exist or is not a directory",
                None,
            ));
        }

        let store =
            fossil_core::storage::GlobalStore::open(&fossil_core::storage::global_db_path())
                .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        let registry = default_registry();

        let watcher = WorkspaceWatcher::start(
            workspace_buf.clone(),
            input.repo_id.clone(),
            store,
            registry,
            std::time::Duration::from_secs(1),
        )
        .map_err(|e| McpError::internal_error(e, None))?;

        {
            let mut map = self.watchers.lock().unwrap();
            if let Some(old) = map.remove(&input.workspace_path) {
                old.stop();
            }
            map.insert(input.workspace_path.clone(), watcher);
        }

        Ok(json!({
            "status": "watching",
            "workspace_path": input.workspace_path,
            "repo_id": input.repo_id,
            "debounce_seconds": 1,
        })
        .to_string())
    }

    #[tool(description = "Stops an active background filesystem watcher on a workspace.")]
    async fn unwatch_workspace(
        &self,
        Parameters(input): Parameters<UnwatchWorkspaceInput>,
    ) -> Result<String, McpError> {
        info!("unwatch_workspace: path={}", input.workspace_path);

        let stopped = {
            let mut map = self.watchers.lock().unwrap();
            if let Some(watcher) = map.remove(&input.workspace_path) {
                watcher.stop();
                true
            } else {
                false
            }
        };

        if stopped {
            Ok(json!({
                "status": "unwatched",
                "workspace_path": input.workspace_path,
            })
            .to_string())
        } else {
            Ok(json!({
                "status": "not_found",
                "message": format!("No active watcher for '{}'", input.workspace_path),
            })
            .to_string())
        }
    }
}

#[tool_handler(
    name = "fossil-mcp",
    version = "0.1.0",
    instructions = "fossil-mcp locates exact implementations inside open-source repos. Workflow: 1) clone_reference to clone a repo, 2) index_repo to parse symbols, 3) locate_implementation to search."
)]
impl ServerHandler for FossilServer {}

#[cfg(test)]
mod tests {
    use super::*;

    // ── helpers ──────────────────────────────────────────────────────────────

    /// Returns true only when the `RUN_E2E` env var is set to "1".
    /// This allows the test suite to skip E2E tests locally by default,
    /// while the CI `e2e` job explicitly sets the variable.
    fn e2e_enabled() -> bool {
        std::env::var("RUN_E2E").as_deref() == Ok("1")
    }

    fn init_tracing() {
        let _ = tracing_subscriber::fmt().with_env_filter("info").try_init();
    }

    // ── E2E: full analyze_feature pipeline ──────────────────────────────────
    //
    // Requires network access (clones https://github.com/dtolnay/itoa).
    // Guarded by `#[ignore]`; the CI `e2e` job runs with
    //   cargo test -- --include-ignored
    // and sets RUN_E2E=1 to actually execute this test.
    //
    #[tokio::test]
    #[ignore = "E2E: requires network; set RUN_E2E=1 and pass --include-ignored"]
    async fn e2e_analyze_feature() {
        if !e2e_enabled() {
            eprintln!("skip: RUN_E2E != 1");
            return;
        }
        init_tracing();

        let server = FossilServer::new();
        let res = server
            .analyze_feature(Parameters(AnalyzeFeatureInput {
                repo_url: "https://github.com/dtolnay/itoa".to_string(),
                query: "write integer to string".to_string(),
            }))
            .await
            .expect("analyze_feature must succeed");

        let val: serde_json::Value =
            serde_json::from_str(&res).expect("response must be valid JSON");
        assert!(
            !val["matches"].as_array().unwrap_or(&vec![]).is_empty(),
            "expected at least one match for 'write integer to string' in itoa"
        );
    }

    // ── E2E: clone + index + search, each step validated separately ─────────
    #[tokio::test]
    #[ignore = "E2E: requires network; set RUN_E2E=1 and pass --include-ignored"]
    async fn e2e_clone_index_search_pipeline() {
        if !e2e_enabled() {
            eprintln!("skip: RUN_E2E != 1");
            return;
        }
        init_tracing();

        let server = FossilServer::new();

        // Step 1 – clone (small, popular Rust crate for reproducibility)
        let clone_res = server
            .clone_reference(Parameters(CloneReferenceInput {
                repo_url: "https://github.com/dtolnay/itoa".to_string(),
                alias: Some("itoa-e2e".to_string()),
                branch: None,
                refresh: Some(false), // re-use cached clone if present
            }))
            .await
            .expect("clone must succeed");

        let clone_val: serde_json::Value =
            serde_json::from_str(&clone_res).expect("clone response must be valid JSON");
        let repo_id = clone_val["repo_id"]
            .as_str()
            .expect("clone response must contain repo_id")
            .to_string();

        // Step 2 – index
        let index_res = server
            .index_repo(Parameters(IndexRepoInput {
                repo_id: repo_id.clone(),
                languages: None,
            }))
            .await
            .expect("index must succeed");

        let index_val: serde_json::Value =
            serde_json::from_str(&index_res).expect("index response must be valid JSON");
        let symbol_count = index_val["symbol_count"].as_u64().unwrap_or(0);
        assert!(
            symbol_count > 0,
            "indexer must extract at least one symbol; got {}",
            symbol_count
        );

        // Step 3 – search
        let search_res = server
            .locate_implementation(Parameters(LocateImplementationInput {
                repo_id: Some(repo_id),
                query: "integer formatting".to_string(),
                top_k: Some(5),
            }))
            .await
            .expect("search must succeed");

        let search_val: serde_json::Value =
            serde_json::from_str(&search_res).expect("search response must be valid JSON");
        assert!(
            !search_val["matches"]
                .as_array()
                .unwrap_or(&vec![])
                .is_empty(),
            "search must return at least one match"
        );
    }
}
