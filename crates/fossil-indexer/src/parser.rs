use fossil_core::types::{CallEdge, Symbol};
use tree_sitter::Tree;

/// A language-specific parser plugin.
///
/// Implementors extract [`Symbol`]s and [`CallEdge`]s from a parsed tree-sitter
/// syntax tree. The trait is object-safe so parsers can be stored as
/// `Box<dyn LanguageParser>` in the [`ParserRegistry`].
pub trait LanguageParser: Send + Sync {
    /// Short lowercase language identifier (e.g. `"rust"`, `"python"`).
    fn language_id(&self) -> &str;

    /// File extensions handled by this parser (without the leading dot).
    fn file_extensions(&self) -> &[&str];

    /// The tree-sitter [`Language`] this parser uses.
    fn ts_language(&self) -> tree_sitter::Language;

    /// Extract all top-level and nested code symbols from the syntax tree.
    fn parse_symbols(&self, source: &[u8], tree: &Tree, file_path: &str) -> Vec<Symbol>;

    /// Extract call edges from the syntax tree (caller name → callee name).
    fn extract_calls(&self, source: &[u8], tree: &Tree, file_path: &str) -> Vec<CallEdge>;
}

/// Registry that maps file extensions to their [`LanguageParser`] implementation.
pub struct ParserRegistry {
    parsers: Vec<Box<dyn LanguageParser>>,
}

impl ParserRegistry {
    pub fn new() -> Self {
        Self { parsers: Vec::new() }
    }

    /// Register a parser. Later registrations for the same extension win.
    pub fn register(&mut self, parser: Box<dyn LanguageParser>) {
        self.parsers.push(parser);
    }

    /// Find a parser for the given file extension (without dot).
    pub fn for_extension(&self, ext: &str) -> Option<&dyn LanguageParser> {
        // Search in reverse so later registrations take precedence.
        self.parsers
            .iter()
            .rev()
            .find(|p| p.file_extensions().contains(&ext))
            .map(|p| p.as_ref())
    }
}

impl Default for ParserRegistry {
    fn default() -> Self {
        Self::new()
    }
}
