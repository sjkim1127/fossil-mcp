pub mod error;
pub mod fuzzy;
pub mod traits;

pub use error::SearchError;
pub use fuzzy::FuzzySearcher;
pub use traits::{SearchResult, Searcher};
