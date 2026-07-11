//! Transitive dependency indexers.
//!
//! Each sub-module handles a specific language's package ecosystem:
//! - `rust_deps`   – Cargo.toml + ~/.cargo/registry
//! - `python_deps` – requirements.txt / pyproject.toml + site-packages  
//! - `js_deps`     – package.json + node_modules
//! - `cpp_deps`    – vcpkg.json / CMakeLists + system headers

pub mod cpp_deps;
pub mod js_deps;
pub mod python_deps;
pub mod rust_deps;

pub use cpp_deps::index_cpp_deps;
pub use js_deps::index_js_deps;
pub use python_deps::index_python_deps;
pub use rust_deps::index_rust_deps;

/// Result of indexing a single external package.
#[derive(Debug, Clone)]
pub struct DepIndexResult {
    pub package_name: String,
    pub package_version: String,
    pub language: String,
    pub source_path: String,
    pub symbol_count: usize,
    /// Whether this was a cache hit (skipped re-indexing).
    pub was_cached: bool,
}
