use std::path::{Path, PathBuf};
use std::str::FromStr;

use chrono::{DateTime, Utc};
use rusqlite::{Connection, params, ffi::sqlite3_auto_extension};
use sqlite_vec::sqlite3_vec_init;
use tracing::{debug, warn};
use std::sync::Once;

use crate::error::StorageError;
use crate::types::{CallEdge, RepoMeta, Symbol, SymbolKind};

/// Manages the SQLite index database for a single repository.
///
/// Each repository gets its own `index.db` file inside its cache directory.
pub struct RepoStore {
    conn: Connection,
}

impl RepoStore {
    /// Open (or create) the index database at `db_path`.
    pub fn open(db_path: &Path) -> Result<Self, StorageError> {
        // Register the sqlite-vec extension globally for all connections (once).
        static INIT_VEC: Once = Once::new();
        INIT_VEC.call_once(|| {
            unsafe {
                sqlite3_auto_extension(Some(std::mem::transmute(sqlite3_vec_init as *const ())));
            }
        });

        let conn = Connection::open(db_path)?;
        // vec0 requires loading it if auto_extension doesn't immediately apply to the very first connection sometimes, but auto_extension applies to newly opened connections.
        
        let store = Self { conn };
        store.init_schema()?;
        Ok(store)
    }

    /// Initialise tables and indices if they don't already exist.
    fn init_schema(&self) -> Result<(), StorageError> {
        self.conn.execute_batch(
            "
            PRAGMA journal_mode = WAL;
            PRAGMA synchronous = NORMAL;

            CREATE TABLE IF NOT EXISTS repos (
                repo_id     TEXT PRIMARY KEY,
                url         TEXT NOT NULL,
                alias       TEXT,
                path        TEXT NOT NULL,
                indexed_at  TEXT,
                symbol_count INTEGER DEFAULT 0
            );

            CREATE TABLE IF NOT EXISTS symbols (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                name        TEXT NOT NULL,
                kind        TEXT NOT NULL,
                file_path   TEXT NOT NULL,
                line_start  INTEGER NOT NULL,
                line_end    INTEGER NOT NULL,
                signature   TEXT NOT NULL,
                language    TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_symbols_name ON symbols(name);
            CREATE INDEX IF NOT EXISTS idx_symbols_language ON symbols(language);

            CREATE TABLE IF NOT EXISTS call_edges (
                id      INTEGER PRIMARY KEY AUTOINCREMENT,
                caller  TEXT NOT NULL,
                callee  TEXT NOT NULL,
                file_path TEXT NOT NULL,
                line    INTEGER NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_call_edges_caller ON call_edges(caller);
            CREATE INDEX IF NOT EXISTS idx_call_edges_callee ON call_edges(callee);
            CREATE INDEX IF NOT EXISTS idx_call_edges_callee ON call_edges(callee);

            -- Vector table for semantic search. We use 384 dimensions for BGE-small-en-v1.5
            CREATE VIRTUAL TABLE IF NOT EXISTS vec_symbols USING vec0(
                symbol_id INTEGER PRIMARY KEY,
                embedding float[384]
            );
            ",
        )?;
        Ok(())
    }

    // ── Repo metadata ───────────────────────────────────────────────────────

    /// Upsert repository metadata.
    pub fn upsert_repo(&self, meta: &RepoMeta) -> Result<(), StorageError> {
        self.conn.execute(
            "INSERT INTO repos (repo_id, url, alias, path, indexed_at, symbol_count)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(repo_id) DO UPDATE SET
                url          = excluded.url,
                alias        = excluded.alias,
                path         = excluded.path,
                indexed_at   = excluded.indexed_at,
                symbol_count = excluded.symbol_count",
            params![
                meta.repo_id,
                meta.url,
                meta.alias,
                meta.path.to_string_lossy(),
                meta.indexed_at.map(|t| t.to_rfc3339()),
                meta.symbol_count as i64,
            ],
        )?;
        Ok(())
    }

    /// Fetch a single repo by id.
    pub fn get_repo(&self, repo_id: &str) -> Result<RepoMeta, StorageError> {
        let meta = self
            .conn
            .query_row(
                "SELECT repo_id, url, alias, path, indexed_at, symbol_count FROM repos WHERE repo_id = ?1",
                params![repo_id],
                |row| {
                    let path: String = row.get(3)?;
                    let indexed_at_str: Option<String> = row.get(4)?;
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        PathBuf::from(path),
                        indexed_at_str,
                        row.get::<_, i64>(5)?,
                    ))
                },
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => {
                    StorageError::RepoNotFound(repo_id.to_string())
                }
                other => StorageError::Sqlite(other),
            })?;

        let indexed_at = meta
            .4
            .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
            .map(|dt| dt.with_timezone(&Utc));

        Ok(RepoMeta {
            repo_id: meta.0,
            url: meta.1,
            alias: meta.2,
            path: meta.3,
            indexed_at,
            symbol_count: meta.5 as u64,
        })
    }

    /// Return all known repositories.
    pub fn list_repos(&self) -> Result<Vec<RepoMeta>, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT repo_id, url, alias, path, indexed_at, symbol_count FROM repos",
        )?;
        let rows = stmt.query_map([], |row| {
            let path: String = row.get(3)?;
            let indexed_at_str: Option<String> = row.get(4)?;
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                PathBuf::from(path),
                indexed_at_str,
                row.get::<_, i64>(5)?,
            ))
        })?;

        let mut result = Vec::new();
        for row in rows {
            let r = row?;
            let indexed_at = r
                .4
                .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                .map(|dt| dt.with_timezone(&Utc));
            result.push(RepoMeta {
                repo_id: r.0,
                url: r.1,
                alias: r.2,
                path: r.3,
                indexed_at,
                symbol_count: r.5 as u64,
            });
        }
        Ok(result)
    }

    // ── Symbols ─────────────────────────────────────────────────────────────

    /// Remove all symbols previously stored (used before re-indexing).
    pub fn clear_symbols(&self) -> Result<(), StorageError> {
        self.conn.execute_batch("
            DELETE FROM symbols;
            DELETE FROM call_edges;
            DELETE FROM vec_symbols;
        ")?;
        Ok(())
    }

    /// Bulk-insert symbols using a transaction.
    pub fn insert_symbols(&self, symbols: &[Symbol]) -> Result<(), StorageError> {
        let tx = self.conn.unchecked_transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO symbols (name, kind, file_path, line_start, line_end, signature, language)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;
            for sym in symbols {
                stmt.execute(params![
                    sym.name,
                    sym.kind.to_string(),
                    sym.file_path,
                    sym.line_start,
                    sym.line_end,
                    sym.signature,
                    sym.language,
                ])?;
            }
        }
        tx.commit()?;
        debug!("Inserted {} symbols", symbols.len());
        Ok(())
    }

    /// Load all symbols for a repository.
    pub fn load_symbols(&self) -> Result<Vec<Symbol>, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, kind, file_path, line_start, line_end, signature, language FROM symbols",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, u32>(4)?,
                row.get::<_, u32>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
            ))
        })?;

        let mut result = Vec::new();
        for row in rows {
            let r = row?;
            let kind = SymbolKind::from_str(&r.2)
                .unwrap_or(SymbolKind::Function);
            result.push(Symbol {
                id: Some(r.0),
                name: r.1,
                kind,
                file_path: r.3,
                line_start: r.4,
                line_end: r.5,
                signature: r.6,
                language: r.7,
            });
        }
        Ok(result)
    }

    // ── Call edges ──────────────────────────────────────────────────────────

    /// Bulk-insert call edges using a transaction.
    pub fn insert_call_edges(&self, edges: &[CallEdge]) -> Result<(), StorageError> {
        let tx = self.conn.unchecked_transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO call_edges (caller, callee, file_path, line) VALUES (?1, ?2, ?3, ?4)",
            )?;
            for edge in edges {
                stmt.execute(params![edge.caller, edge.callee, edge.file_path, edge.line])?;
            }
        }
        tx.commit()?;
        debug!("Inserted {} call edges", edges.len());
        Ok(())
    }

    /// Return all call edges where `caller` matches the given symbol name.
    pub fn calls_made_by(&self, caller: &str) -> Result<Vec<CallEdge>, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT caller, callee, file_path, line FROM call_edges WHERE caller = ?1",
        )?;
        let rows = stmt.query_map(params![caller], |row| {
            Ok(CallEdge {
                caller: row.get(0)?,
                callee: row.get(1)?,
                file_path: row.get(2)?,
                line: row.get(3)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(StorageError::Sqlite)
    }

    /// Return all call edges where `callee` matches the given symbol name.
    pub fn callers_of(&self, callee: &str) -> Result<Vec<CallEdge>, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT caller, callee, file_path, line FROM call_edges WHERE callee = ?1",
        )?;
        let rows = stmt.query_map(params![callee], |row| {
            Ok(CallEdge {
                caller: row.get(0)?,
                callee: row.get(1)?,
                file_path: row.get(2)?,
                line: row.get(3)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(StorageError::Sqlite)
    }

    /// Update symbol_count in the repos table.
    pub fn update_symbol_count(
        &self,
        repo_id: &str,
        count: u64,
        indexed_at: DateTime<Utc>,
    ) -> Result<(), StorageError> {
        self.conn.execute(
            "UPDATE repos SET symbol_count = ?1, indexed_at = ?2 WHERE repo_id = ?3",
            params![count as i64, indexed_at.to_rfc3339(), repo_id],
        )?;
        Ok(())
    }

    /// Insert vector embeddings for semantic search.
    /// Returns the number of inserted vectors.
    pub fn insert_embeddings(&self, embeddings: &[(i64, Vec<f32>)]) -> Result<usize, StorageError> {
        let tx = self.conn.unchecked_transaction()?;
        let mut count = 0;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO vec_symbols (symbol_id, embedding) VALUES (?1, ?2)",
            )?;
            for (id, vec) in embeddings {
                let bytes: &[u8] = bytemuck::cast_slice(vec.as_slice());
                stmt.execute(params![id, bytes])?;
                count += 1;
            }
        }
        tx.commit()?;
        debug!("Inserted {} vector embeddings", count);
        Ok(count)
    }

    /// Perform a vector search using KNN (K-Nearest Neighbors).
    /// Returns a list of symbol_ids and their distance (smaller is closer).
    pub fn search_embeddings(&self, query_embedding: &[f32], limit: usize) -> Result<Vec<(i64, f64)>, StorageError> {
        let mut stmt = self.conn.prepare(
            "
            SELECT symbol_id, distance
            FROM vec_symbols
            WHERE embedding MATCH ?1
            ORDER BY distance
            LIMIT ?2
            "
        )?;
        let bytes: &[u8] = bytemuck::cast_slice(query_embedding);
        let rows = stmt.query_map(params![bytes, limit as i64], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, f64>(1)?,
            ))
        })?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }
}

/// Canonical path for the global fossil-mcp cache directory.
pub fn cache_root() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".fossil-mcp")
        .join("cache")
}

/// Path to the SQLite index file for a given repo cache directory.
pub fn index_db_path(repo_cache_dir: &Path) -> PathBuf {
    repo_cache_dir.join("index.db")
}
