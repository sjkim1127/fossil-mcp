use std::time::Instant;

use chrono::Utc;
use rmcp::{
    handler::server::wrapper::Parameters,
    schemars,
    tool, tool_handler, tool_router,
    ErrorData as McpError, ServerHandler,
};
use serde::Deserialize;
use serde_json::json;
use tracing::{debug, info};

use fossil_core::storage::{RepoStore, index_db_path};
use fossil_indexer::{index_directory, languages::default_registry};
use fossil_repo::{cache::CacheManager, clone::CloneOptions};
use fossil_search::{FuzzySearcher, traits::Searcher};

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
pub struct IndexRepoInput {
    #[schemars(description = "Repository ID returned by clone_reference")]
    pub repo_id: String,
    #[schemars(description = "Language filter e.g. [\"rust\", \"python\"]. Empty means all supported languages.")]
    pub languages: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct LocateImplementationInput {
    #[schemars(description = "Repository ID returned by clone_reference")]
    pub repo_id: String,
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
pub struct ListIndexedReposInput {}

// ── Server ────────────────────────────────────────────────────────────────────

/// The fossil-mcp MCP server.
#[derive(Clone)]
pub struct FossilServer;

impl FossilServer {
    pub fn new() -> Self {
        Self
    }
}

#[tool_router]
impl FossilServer {
    // ── clone_reference ──────────────────────────────────────────────────────

    #[tool(description = "Clone a public git repository into the local fossil-mcp cache. Reuses existing clones unless refresh=true. Returns repo_id needed for subsequent calls.")]
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

        Ok(json!({
            "repo_id": meta.repo_id,
            "path": meta.path.to_string_lossy(),
            "indexed": meta.indexed_at.is_some(),
        })
        .to_string())
    }

    // ── index_repo ───────────────────────────────────────────────────────────

    #[tool(description = "Parse and index all source files in a cloned repository. Extracts symbols (functions, structs, classes, etc.) and builds a 1-hop call graph. Must be run after clone_reference.")]
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

            let (mut symbols, call_edges) =
                index_directory(&repo_dir, &registry).map_err(|e| e.to_string())?;

            // Apply language filter if requested.
            if let Some(ref langs) = languages {
                if !langs.is_empty() {
                    symbols.retain(|s| {
                        langs.iter().any(|l| l.eq_ignore_ascii_case(&s.language))
                    });
                }
            }

            let duration_ms = start.elapsed().as_millis() as u64;
            let files_indexed = symbols
                .iter()
                .map(|s| s.file_path.clone())
                .collect::<std::collections::HashSet<_>>()
                .len();
            let symbol_count = symbols.len() as u64;

            // Persist to SQLite.
            let db_path = index_db_path(&repo_dir);
            let store = RepoStore::open(&db_path).map_err(|e| e.to_string())?;
            store.clear_symbols().map_err(|e| e.to_string())?;
            store.insert_symbols(&symbols).map_err(|e| e.to_string())?;
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

    #[tool(description = "Search for code symbols matching a natural language or keyword query. Returns file paths, line ranges, signatures, and 1-hop related symbols (calls/called_by). Run index_repo first.")]
    async fn locate_implementation(
        &self,
        Parameters(input): Parameters<LocateImplementationInput>,
    ) -> Result<String, McpError> {
        info!(
            "locate_implementation: repo_id={} query={:?}",
            input.repo_id, input.query
        );

        let top_k = input.top_k.unwrap_or(5) as usize;
        let repo_id = input.repo_id.clone();
        let query = input.query.clone();

        let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, String> {
            let repo_dir = CacheManager::repo_dir(&repo_id);
            let db_path = index_db_path(&repo_dir);

            let store = RepoStore::open(&db_path).map_err(|e| e.to_string())?;
            let symbols = store.load_symbols().map_err(|e| e.to_string())?;

            if symbols.is_empty() {
                return Err(format!(
                    "Repository '{}' has no indexed symbols. Run index_repo first.",
                    repo_id
                ));
            }

            let results = FuzzySearcher.search(&query, &symbols, top_k);

            let matches: Vec<serde_json::Value> = results
                .iter()
                .map(|sr| {
                    let sym = &sr.symbol;

                    // Fetch 1-hop related symbols from call edges.
                    let mut related = Vec::new();

                    if let Ok(callees) = store.calls_made_by(&sym.name) {
                        for edge in callees {
                            related.push(json!({
                                "name": edge.callee,
                                "relation": "calls",
                                "file_path": edge.file_path,
                                "line": edge.line,
                            }));
                        }
                    }
                    if let Ok(callers) = store.callers_of(&sym.name) {
                        for edge in callers {
                            related.push(json!({
                                "name": edge.caller,
                                "relation": "called_by",
                                "file_path": edge.file_path,
                                "line": edge.line,
                            }));
                        }
                    }

                    json!({
                        "symbol_name": sym.name,
                        "kind": sym.kind.to_string(),
                        "file_path": sym.file_path,
                        "line_start": sym.line_start,
                        "line_end": sym.line_end,
                        "signature": sym.signature,
                        "score": sr.score,
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

    #[tool(description = "Return the raw source code of a specific file line-range inside a repository. Use file_path and line numbers from locate_implementation results.")]
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

    #[tool(description = "List all repositories present in the fossil-mcp cache, including indexing status.")]
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
}

#[tool_handler(
    name = "fossil-mcp",
    version = "0.1.0",
    instructions = "fossil-mcp locates exact implementations inside open-source repos. Workflow: 1) clone_reference to clone a repo, 2) index_repo to parse symbols, 3) locate_implementation to search."
)]
impl ServerHandler for FossilServer {}
