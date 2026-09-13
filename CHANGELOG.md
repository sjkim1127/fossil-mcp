# Changelog

All notable changes to fossil-mcp will be documented in this file.

Format follows [Keep a Changelog](https://keepachangelog.com/en/1.0.0/).
Releases follow [Semantic Versioning](https://semver.org/).

---

## [Unreleased]

### Added
- Semantic search storage through local SQLite vector tables.
- SCIP index support when `index.scip` is present, with tree-sitter fallback.
- One-shot `analyze_feature` workflow for clone, index, and search.
- Structural migration analysis through `analyze_migration`.
- Local workspace vulnerability pattern scanning through `scan_vulnerabilities`.
- Transitive dependency indexing tools for Rust, Python, JavaScript/TypeScript, and C/C++ package caches.
- Incremental file indexing and workspace watcher tools for local workspaces.

### Changed
- README now documents the full MCP tool surface and current output contracts.
- CI product baseline now includes warning-free Clippy checks.

### Fixed
- Removed dependency-indexer warnings that failed CI with `-D warnings`.
- Ignored macOS Finder metadata files.
- Honored `FOSSIL_CACHE_DIR` and create SQLite parent directories automatically.

---

## [0.1.0] - 2026-07-09

### Added
- **5 MCP tools** via rmcp 2.1 stdio transport:
  - `clone_reference` — shallow-clone a public git repo into `~/.fossil-mcp/cache/`
  - `index_repo` — parse symbols with tree-sitter (Rust, Python, TypeScript) and persist to per-repo SQLite
  - `locate_implementation` — fuzzy-search symbols by name + signature (nucleo-matcher / fzf algorithm)
  - `get_symbol_source` — read raw source for a file/line range
  - `list_indexed_repos` — list cached repos with indexing status
- **1-hop call graph** — `related_symbols` in search results (calls / called_by)
- **3-language support**: Rust (fn, struct, enum, trait, impl), Python (def, class, decorator), TypeScript/JS (function, class, interface, arrow fn)
- **`LanguageParser` trait** — pluggable parser registry for future language additions
- **`Searcher` trait** — pluggable search backend; `FuzzySearcher` is the MVP implementation
- Workspace of 5 crates: `fossil-{core,repo,indexer,search,server}`
- 17 unit tests across all crates

[Unreleased]: https://github.com/sjkim1127/fossil-mcp/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/sjkim1127/fossil-mcp/releases/tag/v0.1.0
