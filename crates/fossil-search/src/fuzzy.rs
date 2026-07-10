use nucleo_matcher::{
    Config, Matcher, Utf32Str,
    pattern::{CaseMatching, Normalization, Pattern},
};

use fossil_core::types::Symbol;

use crate::traits::{SearchResult, Searcher};

/// Fuzzy symbol search powered by [`nucleo_matcher`] (fzf algorithm).
///
/// Matches against the concatenation of `symbol.name` and `symbol.signature`,
/// separated by a space. This lets users find symbols by partial name or by
/// keywords that appear in the parameter list or return type.
pub struct FuzzySearcher;

impl Searcher for FuzzySearcher {
    fn search(&self, query: &str, symbols: &[Symbol], top_k: usize) -> Vec<SearchResult> {
        if query.trim().is_empty() {
            return vec![];
        }

        let mut matcher = Matcher::new(Config::DEFAULT);
        let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);

        let mut scored: Vec<(u32, usize)> = symbols
            .iter()
            .enumerate()
            .filter_map(|(idx, sym)| {
                // Combine name + signature as the match target.
                let haystack = format!("{} {}", sym.name, sym.signature);
                let mut buf = Vec::new();
                let haystack_utf32 = Utf32Str::new(&haystack, &mut buf);
                pattern
                    .score(haystack_utf32, &mut matcher)
                    .map(|score| (score, idx))
            })
            .collect();

        // Sort by score descending.
        scored.sort_by_key(|b| std::cmp::Reverse(b.0));

        let max_score = scored.first().map(|(s, _)| *s).unwrap_or(1).max(1) as f64;

        scored
            .into_iter()
            .take(top_k)
            .map(|(score, idx)| SearchResult {
                symbol: symbols[idx].clone(),
                score: score as f64 / max_score,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fossil_core::types::SymbolKind;

    fn make_sym(name: &str, sig: &str) -> Symbol {
        Symbol {
            id: None,
            repo_id: String::new(),
            name: name.to_string(),
            kind: SymbolKind::Function,
            file_path: "lib.rs".to_string(),
            line_start: 1,
            line_end: 10,
            signature: sig.to_string(),
            language: "rust".to_string(),
        }
    }

    #[test]
    fn finds_best_match() {
        let symbols = vec![
            make_sym(
                "refresh_token",
                "pub fn refresh_token(token: &str) -> Token",
            ),
            make_sym("get_user", "pub fn get_user(id: u32) -> User"),
            make_sym("delete_token", "pub fn delete_token(token: &str)"),
        ];
        let results = FuzzySearcher.search("refresh token", &symbols, 3);
        assert!(!results.is_empty());
        assert_eq!(results[0].symbol.name, "refresh_token");
    }

    #[test]
    fn empty_query_returns_empty() {
        let symbols = vec![make_sym("foo", "fn foo()")];
        let results = FuzzySearcher.search("  ", &symbols, 5);
        assert!(results.is_empty());
    }

    #[test]
    fn top_k_is_respected() {
        let symbols: Vec<_> = (0..20)
            .map(|i| make_sym(&format!("fn_{}", i), ""))
            .collect();
        let results = FuzzySearcher.search("fn", &symbols, 5);
        assert!(results.len() <= 5);
    }
}
