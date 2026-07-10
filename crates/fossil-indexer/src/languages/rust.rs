use fossil_core::types::{CallEdge, Symbol, SymbolKind};
use tree_sitter::{Node, Tree};

use crate::parser::LanguageParser;

/// Extracts Rust symbols: functions, methods, structs, enums, traits, impl blocks.
pub struct RustParser;

impl LanguageParser for RustParser {
    fn language_id(&self) -> &str {
        "rust"
    }

    fn file_extensions(&self) -> &[&str] {
        &["rs"]
    }

    fn ts_language(&self) -> tree_sitter::Language {
        tree_sitter_rust::LANGUAGE.into()
    }

    fn parse_symbols(&self, source: &[u8], tree: &Tree, file_path: &str) -> Vec<Symbol> {
        let mut symbols = Vec::new();
        collect_rust_symbols(source, tree.root_node(), file_path, None, &mut symbols);
        symbols
    }

    fn extract_calls(&self, source: &[u8], tree: &Tree, file_path: &str) -> Vec<CallEdge> {
        let mut edges = Vec::new();
        // We need to know which function we're inside when we encounter a call.
        collect_rust_calls(source, tree.root_node(), file_path, None, &mut edges);
        edges
    }
}

// ── Symbol extraction ────────────────────────────────────────────────────────

fn collect_rust_symbols(
    source: &[u8],
    node: Node<'_>,
    file_path: &str,
    current_impl_type: Option<&str>,
    out: &mut Vec<Symbol>,
) {
    let node_kind = node.kind();

    match node_kind {
        "function_item" => {
            if let Some(sym) = extract_function(source, node, file_path, current_impl_type) {
                out.push(sym);
            }
            // Recurse into the body for nested functions.
            for child in node.children(&mut node.walk()) {
                if child.kind() == "block" {
                    collect_rust_symbols(source, child, file_path, current_impl_type, out);
                }
            }
            return; // Don't fall through to generic recursion.
        }
        "struct_item" => {
            if let Some(sym) = extract_named_item(source, node, file_path, SymbolKind::Struct) {
                out.push(sym);
            }
        }
        "enum_item" => {
            if let Some(sym) = extract_named_item(source, node, file_path, SymbolKind::Enum) {
                out.push(sym);
            }
        }
        "trait_item" => {
            if let Some(sym) = extract_named_item(source, node, file_path, SymbolKind::Trait) {
                out.push(sym);
            }
        }
        "impl_item" => {
            // Extract the type name this impl is for, so methods can carry it.
            let impl_type = extract_impl_type(source, node);
            let impl_type_ref = impl_type.as_deref();
            for child in node.children(&mut node.walk()) {
                collect_rust_symbols(source, child, file_path, impl_type_ref, out);
            }
            return;
        }
        _ => {}
    }

    for child in node.children(&mut node.walk()) {
        collect_rust_symbols(source, child, file_path, current_impl_type, out);
    }
}

fn extract_function(
    source: &[u8],
    node: Node<'_>,
    file_path: &str,
    impl_type: Option<&str>,
) -> Option<Symbol> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(source, name_node);

    let kind = if impl_type.is_some() {
        SymbolKind::Method
    } else {
        SymbolKind::Function
    };

    // Build a signature: everything up to (but not including) the block body.
    let sig = build_function_signature(source, node);

    let start = node.start_position();
    let end = node.end_position();

    Some(Symbol {
        id: None,
        repo_id: String::new(),
        name,
        kind,
        file_path: file_path.to_string(),
        line_start: start.row as u32 + 1,
        line_end: end.row as u32 + 1,
        signature: sig,
        language: "rust".to_string(),
    })
}

fn extract_named_item(
    source: &[u8],
    node: Node<'_>,
    file_path: &str,
    kind: SymbolKind,
) -> Option<Symbol> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(source, name_node);
    let start = node.start_position();
    let end = node.end_position();
    // For structs/enums/traits the signature is the first line.
    let first_line = source_line(source, start.row);

    Some(Symbol {
        id: None,
        repo_id: String::new(),
        name,
        kind,
        file_path: file_path.to_string(),
        line_start: start.row as u32 + 1,
        line_end: end.row as u32 + 1,
        signature: first_line.trim().to_string(),
        language: "rust".to_string(),
    })
}

/// Extract the type name from `impl Foo` or `impl Bar for Baz`.
fn extract_impl_type(source: &[u8], impl_node: Node<'_>) -> Option<String> {
    // The "type" field is the type being implemented.
    if let Some(type_node) = impl_node.child_by_field_name("type") {
        return Some(node_text(source, type_node));
    }
    None
}

/// Build a concise function signature by collecting everything before the body block.
fn build_function_signature(source: &[u8], fn_node: Node<'_>) -> String {
    let mut parts = Vec::new();
    for child in fn_node.children(&mut fn_node.walk()) {
        if child.kind() == "block" {
            break;
        }
        let text = node_text(source, child);
        if !text.is_empty() {
            parts.push(text);
        }
    }
    parts
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

// ── Call edge extraction ─────────────────────────────────────────────────────

fn collect_rust_calls(
    source: &[u8],
    node: Node<'_>,
    file_path: &str,
    current_fn: Option<&str>,
    out: &mut Vec<CallEdge>,
) {
    let kind = node.kind();

    // Track the current enclosing function name.
    let fn_name: Option<String> = if kind == "function_item" {
        node.child_by_field_name("name")
            .map(|n| node_text(source, n))
    } else {
        None
    };

    let caller = fn_name.as_deref().or(current_fn);

    if kind == "call_expression"
        && let Some(callee_name) = extract_call_callee(source, node)
        && let Some(c) = caller
    {
        out.push(CallEdge {
            repo_id: String::new(),
            caller: c.to_string(),
            callee: callee_name,
            file_path: file_path.to_string(),
            line: node.start_position().row as u32 + 1,
        });
    }

    for child in node.children(&mut node.walk()) {
        collect_rust_calls(source, child, file_path, caller, out);
    }
}

/// Extract the callee name from a `call_expression` node.
/// Handles `foo()`, `self.foo()`, `Foo::bar()`, etc.
fn extract_call_callee(source: &[u8], call_node: Node<'_>) -> Option<String> {
    let fn_node = call_node.child_by_field_name("function")?;
    match fn_node.kind() {
        // Simple call: `foo()`
        "identifier" => Some(node_text(source, fn_node)),
        // Method call: `self.foo()` or scoped: `Foo::bar()`
        "field_expression" | "scoped_identifier" => fn_node
            .child_by_field_name("field")
            .or_else(|| fn_node.child_by_field_name("name"))
            .map(|n| node_text(source, n)),
        _ => None,
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

fn node_text(source: &[u8], node: Node<'_>) -> String {
    node.utf8_text(source).unwrap_or("").to_string()
}

fn source_line(source: &[u8], row: usize) -> String {
    let text = std::str::from_utf8(source).unwrap_or("");
    text.lines().nth(row).unwrap_or("").to_string()
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tree_sitter::Parser as TsParser;

    fn parse(src: &str) -> Tree {
        let mut parser = TsParser::new();
        parser
            .set_language(&tree_sitter_rust::LANGUAGE.into())
            .unwrap();
        parser.parse(src, None).unwrap()
    }

    #[test]
    fn extracts_functions() {
        let src = r#"
pub fn hello(x: i32) -> bool {
    true
}
fn private() {}
"#;
        let tree = parse(src);
        let parser = RustParser;
        let symbols = parser.parse_symbols(src.as_bytes(), &tree, "test.rs");
        assert_eq!(symbols.len(), 2);
        assert_eq!(symbols[0].name, "hello");
        assert_eq!(symbols[0].kind, SymbolKind::Function);
        assert_eq!(symbols[1].name, "private");
    }

    #[test]
    fn extracts_struct() {
        let src = "pub struct Foo { x: i32 }";
        let tree = parse(src);
        let symbols = RustParser.parse_symbols(src.as_bytes(), &tree, "test.rs");
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "Foo" && s.kind == SymbolKind::Struct)
        );
    }

    #[test]
    fn extracts_trait() {
        let src = "pub trait Bar { fn baz(&self); }";
        let tree = parse(src);
        let symbols = RustParser.parse_symbols(src.as_bytes(), &tree, "test.rs");
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "Bar" && s.kind == SymbolKind::Trait)
        );
    }

    #[test]
    fn extracts_call_edges() {
        let src = r#"
fn foo() { bar(); }
fn bar() {}
"#;
        let tree = parse(src);
        let edges = RustParser.extract_calls(src.as_bytes(), &tree, "test.rs");
        assert!(edges.iter().any(|e| e.caller == "foo" && e.callee == "bar"));
    }
}
