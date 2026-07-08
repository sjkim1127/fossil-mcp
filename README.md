# fossil-mcp

> **MCP server that locates the exact implementation of any feature inside an open-source repository.**
>
> "Where is OAuth token refresh implemented in this repo?" → exact file, function, line range, related symbols.

## Features

- **Git clone & cache** — shallow clones public repos into `~/.fossil-mcp/cache/`; reuses cache on subsequent runs
- **Multi-language indexing** — extracts functions, structs, classes, traits, interfaces from Rust, Python, TypeScript/JavaScript via tree-sitter
- **Fuzzy symbol search** — matches queries against symbol names and signatures using the fzf algorithm (nucleo-matcher)
- **1-hop call graph** — returns symbols that a matched function calls and that call it
- **5 MCP tools** — `clone_reference`, `index_repo`, `locate_implementation`, `get_symbol_source`, `list_indexed_repos`

## Quick Start

### Build

```bash
git clone https://github.com/sjkim1127/fossil-mcp
cd fossil-mcp
cargo build --release
```

### Connect to Claude Desktop / Cursor / Antigravity

Add to your MCP client config:

```json
{
  "mcpServers": {
    "fossil-mcp": {
      "command": "/path/to/fossil-mcp/target/release/fossil-mcp"
    }
  }
}
```

### Example Usage

```
# In your AI assistant:
Use clone_reference to clone https://github.com/tokio-rs/tokio
Use index_repo with the returned repo_id
Use locate_implementation to search for "spawn blocking"
```

## Workspace Structure

```
fossil-mcp/
├── Cargo.toml                  # workspace root
├── crates/
│   ├── fossil-core/            # shared types, errors, SQLite storage
│   ├── fossil-repo/            # git clone & cache management
│   ├── fossil-indexer/         # tree-sitter parsing + call graph
│   ├── fossil-search/          # fuzzy search (nucleo-matcher)
│   └── fossil-server/          # MCP server entry point (rmcp, stdio)
└── fixtures/                   # test code samples (Rust/Python/TypeScript)
```

## MCP Tools

| Tool | Description |
|---|---|
| `clone_reference` | Clone a public repo; reuse cache or force refresh |
| `index_repo` | Parse symbols & build call graph (run after clone) |
| `locate_implementation` | Fuzzy-search symbols by name/signature |
| `get_symbol_source` | Read raw source for a given file/line range |
| `list_indexed_repos` | List all repos in the local cache |

## Environment Variables

| Variable | Default | Description |
|---|---|---|
| `FOSSIL_LOG` | `warn` | Log level (`fossil_server=debug,warn`) |

## Tech Stack

- **MCP**: `rmcp` 2.1 (stdio transport)
- **Git**: `git2` (libgit2, shallow clone depth=1)
- **Parsing**: `tree-sitter` 0.26 — Rust, Python, TypeScript
- **Search**: `nucleo-matcher` (fzf algorithm)
- **Storage**: `rusqlite` per-repo SQLite database
- **Async**: `tokio`, blocking work via `spawn_blocking`
- **Parallel indexing**: `rayon`
