use fossil_core::types::{CallEdge, Symbol, SymbolKind};
use tree_sitter::{Node, Tree};

use crate::parser::LanguageParser;

/// Extracts TypeScript/JavaScript symbols: functions, classes, methods,
/// interfaces, and arrow function variables.
pub struct TypeScriptParser;

impl LanguageParser for TypeScriptParser {
    fn language_id(&self) -> &str {
        "typescript"
    }

    fn file_extensions(&self) -> &[&str] {
        &["ts", "tsx", "js", "jsx", "mjs", "cjs"]
    }

    fn ts_language(&self) -> tree_sitter::Language {
        tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()
    }

    fn parse_symbols(&self, source: &[u8], tree: &Tree, file_path: &str) -> Vec<Symbol> {
        let mut symbols = Vec::new();
        collect_ts_symbols(source, tree.root_node(), file_path, false, &mut symbols);
        symbols
    }

    fn extract_calls(&self, source: &[u8], tree: &Tree, file_path: &str) -> Vec<CallEdge> {
        let mut edges = Vec::new();
        collect_ts_calls(source, tree.root_node(), file_path, None, &mut edges);
        edges
    }
}

// ── Symbol extraction ────────────────────────────────────────────────────────

fn collect_ts_symbols(
    source: &[u8],
    node: Node<'_>,
    file_path: &str,
    inside_class: bool,
    out: &mut Vec<Symbol>,
) {
    let kind = node.kind();

    match kind {
        "function_declaration" | "function" => {
            if let Some(sym) = extract_ts_function(source, node, file_path, inside_class) {
                out.push(sym);
            }
            // Recurse into body.
            for child in node.children(&mut node.walk()) {
                if child.kind() == "statement_block" {
                    collect_ts_symbols(source, child, file_path, inside_class, out);
                }
            }
            return;
        }
        "method_definition" => {
            if let Some(sym) = extract_ts_function(source, node, file_path, true) {
                out.push(sym);
            }
            return;
        }
        "class_declaration" | "class" => {
            if let Some(sym) = extract_ts_named(source, node, file_path, SymbolKind::Class) {
                out.push(sym);
            }
            for child in node.children(&mut node.walk()) {
                if child.kind() == "class_body" {
                    collect_ts_symbols(source, child, file_path, true, out);
                }
            }
            return;
        }
        "interface_declaration" => {
            if let Some(sym) = extract_ts_named(source, node, file_path, SymbolKind::Interface) {
                out.push(sym);
            }
            return;
        }
        "lexical_declaration" | "variable_declaration" => {
            // Arrow function: `const foo = (x) => x + 1`
            for child in node.children(&mut node.walk()) {
                if child.kind() == "variable_declarator" {
                    if let Some(sym) = extract_arrow_fn(source, child, file_path) {
                        out.push(sym);
                    }
                }
            }
        }
        _ => {}
    }

    for child in node.children(&mut node.walk()) {
        collect_ts_symbols(source, child, file_path, inside_class, out);
    }
}

fn extract_ts_function(
    source: &[u8],
    node: Node<'_>,
    file_path: &str,
    inside_class: bool,
) -> Option<Symbol> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(source, name_node);
    let start = node.start_position();
    let end = node.end_position();
    let sig = build_ts_signature(source, node);

    Some(Symbol { id: None,
        name,
        kind: if inside_class {
            SymbolKind::Method
        } else {
            SymbolKind::Function
        },
        file_path: file_path.to_string(),
        line_start: start.row as u32 + 1,
        line_end: end.row as u32 + 1,
        signature: sig,
        language: "typescript".to_string(),
    })
}

fn extract_ts_named(
    source: &[u8],
    node: Node<'_>,
    file_path: &str,
    kind: SymbolKind,
) -> Option<Symbol> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(source, name_node);
    let start = node.start_position();
    let end = node.end_position();
    let first_line = source_line(source, start.row);

    Some(Symbol { id: None,
        name,
        kind,
        file_path: file_path.to_string(),
        line_start: start.row as u32 + 1,
        line_end: end.row as u32 + 1,
        signature: first_line.trim().to_string(),
        language: "typescript".to_string(),
    })
}

fn extract_arrow_fn(source: &[u8], declarator: Node<'_>, file_path: &str) -> Option<Symbol> {
    let name_node = declarator.child_by_field_name("name")?;
    let value_node = declarator.child_by_field_name("value")?;
    if !matches!(value_node.kind(), "arrow_function" | "function") {
        return None;
    }
    let name = node_text(source, name_node);
    let start = declarator.start_position();
    let end = declarator.end_position();
    let sig = format!("const {} = {}", name, node_text(source, value_node)
        .lines()
        .next()
        .unwrap_or(""));

    Some(Symbol { id: None,
        name,
        kind: SymbolKind::Function,
        file_path: file_path.to_string(),
        line_start: start.row as u32 + 1,
        line_end: end.row as u32 + 1,
        signature: sig,
        language: "typescript".to_string(),
    })
}

fn build_ts_signature(source: &[u8], fn_node: Node<'_>) -> String {
    let mut parts = Vec::new();
    for child in fn_node.children(&mut fn_node.walk()) {
        if child.kind() == "statement_block" {
            break;
        }
        let text = node_text(source, child);
        if !text.is_empty() {
            parts.push(text);
        }
    }
    parts.join(" ").split_whitespace().collect::<Vec<_>>().join(" ")
}

// ── Call edge extraction ─────────────────────────────────────────────────────

fn collect_ts_calls(
    source: &[u8],
    node: Node<'_>,
    file_path: &str,
    current_fn: Option<&str>,
    out: &mut Vec<CallEdge>,
) {
    let node_kind = node.kind();

    let fn_name: Option<String> =
        if matches!(node_kind, "function_declaration" | "method_definition") {
            node.child_by_field_name("name")
                .map(|n| node_text(source, n))
        } else {
            None
        };

    let caller = fn_name.as_deref().or(current_fn);

    if node_kind == "call_expression" {
        if let Some(callee) = extract_ts_callee(source, node) {
            if let Some(c) = caller {
                out.push(CallEdge {
                    caller: c.to_string(),
                    callee,
                    file_path: file_path.to_string(),
                    line: node.start_position().row as u32 + 1,
                });
            }
        }
    }

    for child in node.children(&mut node.walk()) {
        collect_ts_calls(source, child, file_path, caller, out);
    }
}

fn extract_ts_callee(source: &[u8], call_node: Node<'_>) -> Option<String> {
    let fn_node = call_node.child_by_field_name("function")?;
    match fn_node.kind() {
        "identifier" => Some(node_text(source, fn_node)),
        "member_expression" => {
            fn_node
                .child_by_field_name("property")
                .map(|n| node_text(source, n))
        }
        _ => None,
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

fn node_text(source: &[u8], node: Node<'_>) -> String {
    node.utf8_text(source).unwrap_or("").to_string()
}

fn source_line(source: &[u8], row: usize) -> String {
    std::str::from_utf8(source)
        .unwrap_or("")
        .lines()
        .nth(row)
        .unwrap_or("")
        .to_string()
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tree_sitter::Parser as TsParser;

    fn parse(src: &str) -> Tree {
        let mut parser = TsParser::new();
        parser.set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()).unwrap();
        parser.parse(src, None).unwrap()
    }

    #[test]
    fn extracts_function_and_class() {
        let src = r#"
function greet(name: string): string {
    return `Hello ${name}`;
}

class Greeter {
    sayHello(name: string) { return greet(name); }
}
"#;
        let tree = parse(src);
        let symbols = TypeScriptParser.parse_symbols(src.as_bytes(), &tree, "test.ts");
        let names: Vec<_> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"greet"));
        assert!(names.contains(&"Greeter"));
        assert!(names.contains(&"sayHello"));
    }

    #[test]
    fn extracts_interface() {
        let src = "interface Repo { id: string; }";
        let tree = parse(src);
        let symbols = TypeScriptParser.parse_symbols(src.as_bytes(), &tree, "test.ts");
        assert!(symbols.iter().any(|s| s.name == "Repo" && s.kind == SymbolKind::Interface));
    }

    #[test]
    fn extracts_call_edges() {
        let src = r#"
function foo() { bar(); }
function bar() {}
"#;
        let tree = parse(src);
        let edges = TypeScriptParser.extract_calls(src.as_bytes(), &tree, "test.ts");
        assert!(edges.iter().any(|e| e.caller == "foo" && e.callee == "bar"));
    }
}
