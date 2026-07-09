use std::collections::HashMap;

use fossil_core::types::CallEdge;

/// Relation direction from the perspective of the queried symbol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Relation {
    /// The queried symbol calls this one.
    Calls,
    /// This symbol calls the queried one.
    CalledBy,
}

/// A related symbol discovered via the 1-hop call graph.
#[derive(Debug, Clone)]
pub struct RelatedSymbol {
    pub name: String,
    pub relation: Relation,
    /// File where the call site appears (relative to repo root).
    pub file_path: String,
    pub line: u32,
}

/// An in-memory 1-hop call graph built from extracted [`CallEdge`]s.
///
/// This is a name-based (not type-resolved) graph, so there may be
/// false positives when multiple symbols share the same name.
pub struct CallGraph {
    /// caller_name → list of edges that originate from it
    by_caller: HashMap<String, Vec<CallEdge>>,
    /// callee_name → list of edges that target it
    by_callee: HashMap<String, Vec<CallEdge>>,
}

impl CallGraph {
    /// Build a `CallGraph` from a flat list of edges.
    pub fn build(edges: Vec<CallEdge>) -> Self {
        let mut by_caller: HashMap<String, Vec<CallEdge>> = HashMap::new();
        let mut by_callee: HashMap<String, Vec<CallEdge>> = HashMap::new();

        for edge in edges {
            by_callee
                .entry(edge.callee.clone())
                .or_default()
                .push(edge.clone());
            by_caller.entry(edge.caller.clone()).or_default().push(edge);
        }

        Self {
            by_caller,
            by_callee,
        }
    }

    /// Return 1-hop related symbols: what `symbol_name` calls + what calls it.
    pub fn related(&self, symbol_name: &str) -> Vec<RelatedSymbol> {
        let mut result = Vec::new();

        // Functions this symbol calls.
        if let Some(edges) = self.by_caller.get(symbol_name) {
            for edge in edges {
                result.push(RelatedSymbol {
                    name: edge.callee.clone(),
                    relation: Relation::Calls,
                    file_path: edge.file_path.clone(),
                    line: edge.line,
                });
            }
        }

        // Functions that call this symbol.
        if let Some(edges) = self.by_callee.get(symbol_name) {
            for edge in edges {
                result.push(RelatedSymbol {
                    name: edge.caller.clone(),
                    relation: Relation::CalledBy,
                    file_path: edge.file_path.clone(),
                    line: edge.line,
                });
            }
        }

        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edge(caller: &str, callee: &str) -> CallEdge {
        CallEdge {
            repo_id: String::new(),
            caller: caller.to_string(),
            callee: callee.to_string(),
            file_path: "".to_string(),
            line: 1,
        }
    }

    #[test]
    fn graph_finds_callees() {
        let graph = CallGraph::build(vec![edge("foo", "bar"), edge("foo", "baz")]);
        let related = graph.related("foo");
        let callees: Vec<_> = related
            .iter()
            .filter(|r| r.relation == Relation::Calls)
            .map(|r| r.name.as_str())
            .collect();
        assert!(callees.contains(&"bar"));
        assert!(callees.contains(&"baz"));
    }

    #[test]
    fn graph_finds_callers() {
        let graph = CallGraph::build(vec![edge("foo", "bar"), edge("qux", "bar")]);
        let related = graph.related("bar");
        let callers: Vec<_> = related
            .iter()
            .filter(|r| r.relation == Relation::CalledBy)
            .map(|r| r.name.as_str())
            .collect();
        assert!(callers.contains(&"foo"));
        assert!(callers.contains(&"qux"));
    }
}
