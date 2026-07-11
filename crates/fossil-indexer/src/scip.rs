use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::Path;

use protobuf::Message;
use scip::types::Index;

use crate::error::IndexError;
use fossil_core::types::{CallEdge, Symbol, SymbolKind};

/// Parse an `index.scip` Protobuf file into a list of Symbols and CallEdges.
pub fn parse_scip_index(
    path: &Path,
    repo_id: &str,
) -> Result<(Vec<Symbol>, Vec<CallEdge>), IndexError> {
    let mut file = File::open(path).map_err(IndexError::Io)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(IndexError::Io)?;

    let index = Index::parse_from_bytes(&bytes).map_err(|e| IndexError::ParseFailed {
        file: path.display().to_string(),
        message: format!("Failed to parse SCIP Protobuf: {}", e),
    })?;

    let mut symbols = Vec::new();
    let mut edges = Vec::new();

    // Map from SCIP symbol string to its parsed Fossil Symbol for later reference.
    // SCIP symbols look like "rust-analyzer cargo package_name version module/path/FunctionName()."
    let mut symbol_map: HashMap<String, Symbol> = HashMap::new();

    for doc in &index.documents {
        let file_path = doc.relative_path.clone();

        let mut info_map = HashMap::new();
        for sym_info in &doc.symbols {
            info_map.insert(sym_info.symbol.clone(), sym_info);
        }

        let mut def_occurrences = Vec::new();

        for occ in &doc.occurrences {
            let is_definition = (occ.symbol_roles & 1) != 0; // 1 = Definition role
            if is_definition {
                def_occurrences.push(occ);
                let sym_str = &occ.symbol;

                let kind = SymbolKind::Function;
                let mut signature = String::new();
                let mut display_name = sym_str
                    .split('.')
                    .next_back()
                    .unwrap_or(sym_str)
                    .to_string();
                if display_name.ends_with("().") || display_name.ends_with("()") {
                    display_name = display_name
                        .trim_end_matches("().")
                        .trim_end_matches("()")
                        .to_string();
                }

                if let Some(info) = info_map.get(sym_str) {
                    if !info.display_name.is_empty() {
                        display_name = info.display_name.clone();
                    }
                    if let Some(sig) = info.signature_documentation.as_ref() {
                        signature = sig.text.clone();
                    }
                }

                let line_start = occ.range.first().copied().unwrap_or(0) as u32 + 1;
                let line_end = if occ.range.len() == 4 {
                    occ.range.get(2).copied().unwrap_or(0) as u32 + 1
                } else {
                    line_start
                };

                let sym = Symbol {
                    id: None,
                    repo_id: repo_id.to_string(),
                    name: display_name,
                    kind,
                    file_path: file_path.clone(),
                    line_start,
                    line_end,
                    signature,
                    language: doc.language.clone(),
                    source: fossil_core::SymbolSource::default(),
                    package_name: None,
                    package_version: None,
                };
                symbol_map.insert(sym_str.clone(), sym.clone());
                symbols.push(sym);
            }
        }

        for occ in &doc.occurrences {
            let is_definition = (occ.symbol_roles & 1) != 0;
            if !is_definition {
                let ref_line = occ.range.first().copied().unwrap_or(0) as u32 + 1;

                let mut enclosing_def = None;
                for def in &def_occurrences {
                    let d_start = def.range.first().copied().unwrap_or(0) as u32 + 1;
                    let d_end = if def.range.len() == 4 {
                        def.range.get(2).copied().unwrap_or(0) as u32 + 1
                    } else {
                        d_start
                    };

                    if ref_line >= d_start && ref_line <= d_end {
                        enclosing_def = Some(def);
                    }
                }

                if let Some(caller_def) = enclosing_def {
                    let mut caller_name = caller_def
                        .symbol
                        .split('.')
                        .next_back()
                        .unwrap_or(&caller_def.symbol)
                        .to_string();
                    let mut callee_name = occ
                        .symbol
                        .split('.')
                        .next_back()
                        .unwrap_or(&occ.symbol)
                        .to_string();

                    if caller_name.ends_with("().") || caller_name.ends_with("()") {
                        caller_name = caller_name
                            .trim_end_matches("().")
                            .trim_end_matches("()")
                            .to_string();
                    }
                    if callee_name.ends_with("().") || callee_name.ends_with("()") {
                        callee_name = callee_name
                            .trim_end_matches("().")
                            .trim_end_matches("()")
                            .to_string();
                    }

                    edges.push(CallEdge {
                        repo_id: repo_id.to_string(),
                        caller: caller_name,
                        callee: callee_name,
                        file_path: file_path.clone(),
                        line: ref_line,
                    });
                }
            }
        }
    }

    Ok((symbols, edges))
}
