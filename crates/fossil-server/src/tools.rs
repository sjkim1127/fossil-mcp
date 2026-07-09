use std::time::Instant;

use chrono::Utc;
use rmcp::{
    ErrorData as McpError, ServerHandler, handler::server::wrapper::Parameters, schemars, tool,
    tool_handler, tool_router,
};
use serde::Deserialize;
use serde_json::json;
use tracing::{debug, info};

use fossil_core::storage::GlobalStore;
use fossil_indexer::{index_directory, languages::default_registry, parse_scip_index};
use fossil_repo::{cache::CacheManager, clone::CloneOptions};
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
            if let Some(ref langs) = languages {
                if !langs.is_empty() {
                    symbols.retain(|s| langs.iter().any(|l| l.eq_ignore_ascii_case(&s.language)));
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
            {
                if let Some(query_embed) = query_embeds.pop() {
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

    #[tokio::test]
    async fn test_analyze_feature_integration() {
        let _ = tracing_subscriber::fmt()
            .with_env_filter("debug")
            .try_init();
        let server = FossilServer::new();
        let res = server
            .analyze_feature(Parameters(AnalyzeFeatureInput {
                repo_url: "https://github.com/dtolnay/itoa".to_string(),
                query: "write integer to string".to_string(),
            }))
            .await
            .unwrap();

        println!("Result: {}", res);

        let val: serde_json::Value = serde_json::from_str(&res).unwrap();
        assert!(val["matches"].as_array().unwrap().len() > 0);
    }
}
