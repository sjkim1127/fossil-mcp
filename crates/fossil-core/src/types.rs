use serde::{Deserialize, Serialize};
use chrono::{DateTime, Utc};
use std::path::PathBuf;

/// The kind of a code symbol.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SymbolKind {
    Function,
    Method,
    Struct,
    Class,
    Trait,
    Interface,
    Enum,
    Module,
}

impl std::fmt::Display for SymbolKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            SymbolKind::Function => "function",
            SymbolKind::Method => "method",
            SymbolKind::Struct => "struct",
            SymbolKind::Class => "class",
            SymbolKind::Trait => "trait",
            SymbolKind::Interface => "interface",
            SymbolKind::Enum => "enum",
            SymbolKind::Module => "module",
        };
        write!(f, "{}", s)
    }
}

impl std::str::FromStr for SymbolKind {
    type Err = crate::CoreError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "function" => Ok(SymbolKind::Function),
            "method" => Ok(SymbolKind::Method),
            "struct" => Ok(SymbolKind::Struct),
            "class" => Ok(SymbolKind::Class),
            "trait" => Ok(SymbolKind::Trait),
            "interface" => Ok(SymbolKind::Interface),
            "enum" => Ok(SymbolKind::Enum),
            "module" => Ok(SymbolKind::Module),
            other => Err(crate::CoreError::InvalidSymbolKind(other.to_string())),
        }
    }
}

/// A code symbol extracted from a source file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Symbol {
    pub name: String,
    pub kind: SymbolKind,
    /// Path relative to the repo root.
    pub file_path: String,
    pub line_start: u32,
    pub line_end: u32,
    /// Full signature text (e.g. `pub fn foo(x: i32) -> bool`).
    pub signature: String,
    /// Language identifier (e.g. "rust", "python", "typescript").
    pub language: String,
}

/// A directed call edge between two symbols.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallEdge {
    /// Name of the calling symbol.
    pub caller: String,
    /// Name of the callee symbol.
    pub callee: String,
    /// File where the call site appears (relative to repo root).
    pub file_path: String,
    /// Line number of the call site.
    pub line: u32,
}

/// Metadata for a cloned repository.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoMeta {
    /// SHA-256(url)[..16] hex string used as a stable identifier.
    pub repo_id: String,
    pub url: String,
    pub alias: Option<String>,
    pub path: PathBuf,
    pub indexed_at: Option<DateTime<Utc>>,
    pub symbol_count: u64,
}

/// A single match from a search query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub symbol: Symbol,
    /// Normalised match score in [0.0, 1.0].
    pub score: f64,
}
