use std::collections::hash_map::Entry;
use std::collections::HashMap;

use zti_common::dsl::SymbolBodyEntry;

use crate::model::ProjectIndex;

pub fn resolve_symbol_bodies(
    index: &ProjectIndex,
    symbol_ids: &[u32],
) -> Vec<SymbolBodyEntry> {
    let mut entries = Vec::with_capacity(symbol_ids.len());
    let mut file_cache: HashMap<u16, Result<String, String>> =
        HashMap::with_capacity(symbol_ids.len());

    for &id in symbol_ids {
        let sym = match index.symbols.get(id as usize) {
            Some(s) => s,
            None => {
                entries.push(SymbolBodyEntry::Err {
                    symbol_id: id,
                    message: format!("Symbol {} not found", id),
                });
                continue;
            }
        };

        let file = match index.files.get(sym.file_idx as usize) {
            Some(f) => f,
            None => {
                entries.push(SymbolBodyEntry::Err {
                    symbol_id: id,
                    message: format!("File for symbol {} not found", id),
                });
                continue;
            }
        };

        let content = match file_cache.entry(sym.file_idx) {
            Entry::Occupied(e) => e.get().clone(),
            Entry::Vacant(e) => {
                let result = std::fs::read_to_string(&file.path)
                    .map_err(|err| format!("Failed to read {}: {}", file.path, err));
                e.insert(result.clone());
                result
            }
        };

        match content {
            Ok(ref c) => {
                let range = zti_common::line_byte_range(c, sym.line, sym.end_line);
                entries.push(SymbolBodyEntry::Ok {
                    symbol_id: id,
                    kind_short: sym.kind.short().to_owned(),
                    start_line: sym.line,
                    end_line: sym.end_line,
                    body: c[range].to_owned(),
                });
            }
            Err(msg) => {
                entries.push(SymbolBodyEntry::Err {
                    symbol_id: id,
                    message: msg,
                });
            }
        }
    }

    entries
}
