pub mod rust;
pub mod python;
pub mod typescript;

pub use rust::RustParser;
pub use python::PythonParser;
pub use typescript::TypeScriptParser;

use crate::parser::ParserRegistry;

/// Create a [`ParserRegistry`] pre-loaded with all built-in language parsers.
pub fn default_registry() -> ParserRegistry {
    let mut registry = ParserRegistry::new();
    registry.register(Box::new(RustParser));
    registry.register(Box::new(PythonParser));
    registry.register(Box::new(TypeScriptParser));
    registry
}
