use serde::{Deserialize, Serialize};
use tree_sitter::{Node, Parser};

use crate::parser::ParserRegistry;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationPattern {
    pub file_path: String,
    pub before_snippet: String,
    pub after_snippet: String,
}

fn is_structural_node(kind: &str) -> bool {
    matches!(
        kind,
        "function_item"
            | "function_declaration"
            | "class_declaration"
            | "method_definition"
            | "impl_item"
            | "lexical_declaration"
            | "variable_declaration"
            | "interface_declaration"
            | "type_alias_declaration"
    )
}

fn find_encompassing_node<'a>(node: Node<'a>, line_1_indexed: usize) -> Option<Node<'a>> {
    let row = line_1_indexed.saturating_sub(1);

    let start = node.start_position().row;
    let end = node.end_position().row;

    if row < start || row > end {
        return None;
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if let Some(n) = find_encompassing_node(child, line_1_indexed) {
            // We found a deeper structural node
            return Some(n);
        }
    }

    if is_structural_node(node.kind()) {
        Some(node)
    } else {
        None
    }
}

pub fn extract_structural_diff(
    registry: &ParserRegistry,
    file_path: &str,
    before_src: &str,
    after_src: &str,
    changed_lines_before: &[usize],
    changed_lines_after: &[usize],
) -> Option<MigrationPattern> {
    let ext = std::path::Path::new(file_path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");

    let lang_parser = registry.for_extension(ext)?;

    let mut parser = Parser::new();
    // Safety check because tree-sitter language might fail to set
    if parser.set_language(&lang_parser.ts_language()).is_err() {
        return None;
    }

    let before_tree = parser.parse(before_src, None)?;
    let after_tree = parser.parse(after_src, None)?;

    let mut before_nodes = Vec::new();
    for &line in changed_lines_before {
        if let Some(node) = find_encompassing_node(before_tree.root_node(), line) {
            before_nodes.push(node);
        }
    }

    let mut after_nodes = Vec::new();
    for &line in changed_lines_after {
        if let Some(node) = find_encompassing_node(after_tree.root_node(), line) {
            after_nodes.push(node);
        }
    }

    let before_snippet = if let Some(n) = before_nodes.first() {
        n.utf8_text(before_src.as_bytes()).unwrap_or("").to_string()
    } else {
        String::new()
    };

    let after_snippet = if let Some(n) = after_nodes.first() {
        n.utf8_text(after_src.as_bytes()).unwrap_or("").to_string()
    } else {
        String::new()
    };

    if before_snippet.is_empty() && after_snippet.is_empty() {
        return None;
    }

    Some(MigrationPattern {
        file_path: file_path.to_string(),
        before_snippet,
        after_snippet,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::languages::typescript::TypeScriptParser;

    #[test]
    fn test_extract_structural_diff() {
        let mut registry = ParserRegistry::new();
        registry.register(Box::new(TypeScriptParser));

        let before = r#"
class Button extends React.Component {
    render() {
        return <button>Click</button>;
    }
}
"#;
        let after = r#"
const Button = () => {
    return <button>Click</button>;
};
"#;

        // Line 2 was changed in 'before', Line 2 was added in 'after'
        let pattern =
            extract_structural_diff(&registry, "test.tsx", before, after, &[2, 3, 4], &[2, 3])
                .unwrap();

        println!("Before snippet:\n{}", pattern.before_snippet);
        println!("After snippet:\n{}", pattern.after_snippet);

        assert!(pattern.before_snippet.contains("class Button"));
        assert!(pattern.after_snippet.contains("const Button = () =>"));
    }
}
