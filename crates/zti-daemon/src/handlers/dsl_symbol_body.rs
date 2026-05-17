use zti_protocol::request::SymbolBodyReq;
use zti_protocol::response::{Response, SymbolBodyBody};

use crate::handlers::dsl_file_tree::ensure_dsl_index;
use crate::handlers::with_project;
use crate::state::DaemonState;

pub async fn handle(req: &SymbolBodyReq, state: &DaemonState) -> Response {
    let project_root = req.project_root.clone();
    let symbol_id = req.symbol_id;

    let result = with_project(state, &req.project_root, |project| async move {
        let index = ensure_dsl_index(&project, &project_root).await?;

        let sym = index
            .symbols
            .get(symbol_id as usize)
            .ok_or_else(|| anyhow::anyhow!("Symbol {} not found", symbol_id))?;
        let file = index
            .files
            .get(sym.file_idx as usize)
            .ok_or_else(|| anyhow::anyhow!("File for symbol {} not found", symbol_id))?;

        let content = std::fs::read_to_string(&file.path)
            .map_err(|e| anyhow::anyhow!("Failed to read {}: {}", file.path, e))?;

        let range = zti_common::line_byte_range(&content, sym.line, sym.end_line);
        let body = &content[range];
        let text = format!(
            "// File: {} | Lines: {}-{}\n{}",
            file.path, sym.line, sym.end_line, body
        );

        Ok(SymbolBodyBody { text })
    })
    .await;

    Response::DslSymbolBody(result)
}
