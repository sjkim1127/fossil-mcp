use thiserror::Error;

/// Errors produced by the search engine.
#[derive(Debug, Error)]
pub enum SearchError {
    #[error("invalid query: {0}")]
    InvalidQuery(String),
    #[error("internal search error: {0}")]
    Internal(String),
}
