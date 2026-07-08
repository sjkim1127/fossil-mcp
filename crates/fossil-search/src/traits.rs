use fossil_core::types::Symbol;

/// A single search result.
#[derive(Debug, Clone)]
pub struct SearchResult {
    pub symbol: Symbol,
    /// Normalised score in [0.0, 1.0]. Higher is better.
    pub score: f64,
}

/// Abstraction over search back-ends.
///
/// MVP provides [`crate::fuzzy::FuzzySearcher`].
/// Future implementations (e.g. tantivy full-text) can be plugged in by
/// implementing this trait.
pub trait Searcher: Send + Sync {
    /// Search `symbols` using `query` and return up to `top_k` results.
    fn search(&self, query: &str, symbols: &[Symbol], top_k: usize) -> Vec<SearchResult>;
}
