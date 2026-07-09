use fossil_core::types::{CallEdge, Symbol, SymbolKind};
use tree_sitter::{Node, Tree};

use crate::parser::LanguageParser;

/// Extracts Python symbols: functions, classes, methods.
pub struct PythonParser;

impl LanguageParser for PythonParser {
    fn language_id(&self) -> &str {
        "python"
    }

    fn file_extensions(&self) -> &[&str] {
        &["py"]
    }

    fn ts_language(&self) -> tree_sitter::Language {
        tree_sitter_python::LANGUAGE.into()
    }

    fn parse_symbols(&self, source: &[u8], tree: &Tree, file_path: &str) -> Vec<Symbol> {
        let mut symbols = Vec::new();
        collect_python_symbols(source, tree.root_node(), file_path, false, &mut symbols);
        symbols
    }

    fn extract_calls(&self, source: &[u8], tree: &Tree, file_path: &str) -> Vec<CallEdge> {
        let mut edges = Vec::new();
        collect_python_calls(source, tree.root_node(), file_path, None, &mut edges);
        edges
    }
}

// ── Symbol extraction ────────────────────────────────────────────────────────

fn collect_python_symbols(
    source: &[u8],
    node: Node<'_>,
    file_path: &str,
    inside_class: bool,
    out: &mut Vec<Symbol>,
) {
    match node.kind() {
        "function_definition" | "async_function_def" => {
            if let Some(sym) = extract_python_function(source, node, file_path, inside_class) {
                out.push(sym);
            }
            // Recurse into function body for nested defs.
            for child in node.children(&mut node.walk()) {
                if child.kind() == "block" {
                    collect_python_symbols(source, child, file_path, inside_class, out);
                }
            }
            return;
        }
        "class_definition" => {
            if let Some(sym) = extract_python_class(source, node, file_path) {
                out.push(sym);
            }
            // Recurse inside class body, marking methods as inside_class=true.
            for child in node.children(&mut node.walk()) {
                if child.kind() == "block" {
                    collect_python_symbols(source, child, file_path, true, out);
                }
            }
            return;
        }
        "decorated_definition" => {
            // Walk through decorators to the underlying function/class.
            for child in node.children(&mut node.walk()) {
                if matches!(
                    child.kind(),
                    "function_definition" | "class_definition" | "async_function_def"
                ) {
                    collect_python_symbols(source, child, file_path, inside_class, out);
                }
            }
            return;
        }
        _ => {}
    }

    for child in node.children(&mut node.walk()) {
        collect_python_symbols(source, child, file_path, inside_class, out);
    }
}

fn extract_python_function(
    source: &[u8],
    node: Node<'_>,
    file_path: &str,
    inside_class: bool,
) -> Option<Symbol> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(source, name_node);
    let start = node.start_position();
    let end = node.end_position();
    let sig = build_python_signature(source, node);

    Some(Symbol {
        id: None,
        repo_id: String::new(),
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
        language: "python".to_string(),
    })
}

fn extract_python_class(source: &[u8], node: Node<'_>, file_path: &str) -> Option<Symbol> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(source, name_node);
    let start = node.start_position();
    let end = node.end_position();
    let first_line = source_line(source, start.row);

    Some(Symbol {
        id: None,
        repo_id: String::new(),
        name,
        kind: SymbolKind::Class,
        file_path: file_path.to_string(),
        line_start: start.row as u32 + 1,
        line_end: end.row as u32 + 1,
        signature: first_line.trim().to_string(),
        language: "python".to_string(),
    })
}

fn build_python_signature(source: &[u8], fn_node: Node<'_>) -> String {
    // Grab the `def name(params) -> ret:` part (everything before the body block).
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

fn collect_python_calls(
    source: &[u8],
    node: Node<'_>,
    file_path: &str,
    current_fn: Option<&str>,
    out: &mut Vec<CallEdge>,
) {
    let node_kind = node.kind();

    let fn_name: Option<String> =
        if matches!(node_kind, "function_definition" | "async_function_def") {
            node.child_by_field_name("name")
                .map(|n| node_text(source, n))
        } else {
            None
        };

    let caller = fn_name.as_deref().or(current_fn);

    if node_kind == "call" {
        if let Some(callee) = extract_python_callee(source, node) {
            if let Some(c) = caller {
                out.push(CallEdge {
                    repo_id: String::new(),
                    caller: c.to_string(),
                    callee,
                    file_path: file_path.to_string(),
                    line: node.start_position().row as u32 + 1,
                });
            }
        }
    }

    for child in node.children(&mut node.walk()) {
        collect_python_calls(source, child, file_path, caller, out);
    }
}

fn extract_python_callee(source: &[u8], call_node: Node<'_>) -> Option<String> {
    let fn_node = call_node.child_by_field_name("function")?;
    match fn_node.kind() {
        "identifier" => Some(node_text(source, fn_node)),
        "attribute" => fn_node
            .child_by_field_name("attribute")
            .map(|n| node_text(source, n)),
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
        parser
            .set_language(&tree_sitter_python::LANGUAGE.into())
            .unwrap();
        parser.parse(src, None).unwrap()
    }

    #[test]
    fn extracts_functions_and_classes() {
        let src = r#"
def hello(x: int) -> bool:
    return True

class Foo:
    def method(self):
        pass
"#;
        let tree = parse(src);
        let symbols = PythonParser.parse_symbols(src.as_bytes(), &tree, "test.py");
        let names: Vec<_> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"hello"));
        assert!(names.contains(&"Foo"));
        assert!(names.contains(&"method"));

        let method = symbols.iter().find(|s| s.name == "method").unwrap();
        assert_eq!(method.kind, SymbolKind::Method);
    }

    #[test]
    fn extracts_call_edges() {
        let src = r#"
def foo():
    bar()

def bar():
    pass
"#;
        let tree = parse(src);
        let edges = PythonParser.extract_calls(src.as_bytes(), &tree, "test.py");
        assert!(edges.iter().any(|e| e.caller == "foo" && e.callee == "bar"));
    }
}
