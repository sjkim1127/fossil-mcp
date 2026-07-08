# Changelog

All notable changes to fossil-mcp will be documented in this file.

Format follows [Keep a Changelog](https://keepachangelog.com/en/1.0.0/).
Releases follow [Semantic Versioning](https://semver.org/).

---

## [Unreleased]

### Added
- Nothing yet.

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
