use std::path::Path;

use rayon::prelude::*;
use tracing::{debug, warn};
use tree_sitter::Parser as TsParser;
use walkdir::WalkDir;

use fossil_core::types::{CallEdge, Symbol};

use crate::error::IndexError;
use crate::parser::ParserRegistry;

type ParseResult = Result<(Vec<Symbol>, Vec<CallEdge>), IndexError>;

/// Walk `repo_dir`, parse every source file with the appropriate language parser,
/// and return the collected symbols and call edges.
///
/// Files are processed in parallel via Rayon; results are merged afterward.
pub fn index_directory(
    repo_dir: &Path,
    repo_id: &str,
    registry: &ParserRegistry,
) -> Result<(Vec<Symbol>, Vec<CallEdge>), IndexError> {
    // Collect all source files first (single-threaded walk is fast enough).
    let files: Vec<_> = WalkDir::new(repo_dir)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter_map(|e| {
            let path = e.into_path();
            let ext = path.extension()?.to_str()?;
            // Only keep files whose extension has a registered parser.
            if registry.for_extension(ext).is_some() {
                Some(path)
            } else {
                None
            }
        })
        .collect();

    debug!("Found {} indexable files in {:?}", files.len(), repo_dir);

    // Parallel parse.
    let results: Vec<ParseResult> = files
        .par_iter()
        .map(|path| parse_file(path, repo_dir, repo_id, registry))
        .collect();

    let mut all_symbols = Vec::new();
    let mut all_edges = Vec::new();
    let mut errors = 0usize;

    for result in results {
        match result {
            Ok((syms, edges)) => {
                all_symbols.extend(syms);
                all_edges.extend(edges);
            }
            Err(e) => {
                warn!("Parse error (skipping): {}", e);
                errors += 1;
            }
        }
    }

    if errors > 0 {
        warn!("{} file(s) had parse errors and were skipped", errors);
    }

    Ok((all_symbols, all_edges))
}

/// Parse a single file and return its symbols and call edges.
fn parse_file(
    path: &Path,
    repo_dir: &Path,
    repo_id: &str,
    registry: &ParserRegistry,
) -> Result<(Vec<Symbol>, Vec<CallEdge>), IndexError> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");

    let lang_parser = registry
        .for_extension(ext)
        .ok_or_else(|| IndexError::UnsupportedLanguage(ext.to_string()))?;

    let source = std::fs::read(path)?;

    let mut ts_parser = TsParser::new();
    ts_parser
        .set_language(&lang_parser.ts_language())
        .map_err(|e| IndexError::ParseFailed {
            file: path.display().to_string(),
            message: e.to_string(),
        })?;

    let tree = ts_parser
        .parse(&source, None)
        .ok_or_else(|| IndexError::ParseFailed {
            file: path.display().to_string(),
            message: "tree-sitter returned None".to_string(),
        })?;

    // Use a repo-root-relative path in all output.
    let rel_path = path
        .strip_prefix(repo_dir)
        .unwrap_or(path)
        .to_string_lossy()
        .to_string();

    let mut symbols = lang_parser.parse_symbols(&source, &tree, &rel_path);
    let mut edges = lang_parser.extract_calls(&source, &tree, &rel_path);

    for sym in &mut symbols {
        sym.repo_id = repo_id.to_string();
    }
    for edge in &mut edges {
        edge.repo_id = repo_id.to_string();
    }

    Ok((symbols, edges))
}
