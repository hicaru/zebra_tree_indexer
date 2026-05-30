use zti_protocol::request::SearchDepReq;
use zti_protocol::response::{Response, SearchDepBody};

use crate::handlers::dsl_file_tree::ensure_dsl_index;
use crate::handlers::with_project;
use crate::state::DaemonState;

pub async fn handle(req: &SearchDepReq, state: &DaemonState) -> Response {
    let project_root = req.project_root.clone();
    let name = req.name.clone();
    let max_tokens = req.max_tokens.unwrap_or(6000);

    let depth = req.depth.unwrap_or(2);
    let result = with_project(state, &req.project_root, |project| async move {
        let index = ensure_dsl_index(&project, &project_root).await?;
        let text = match zti_dsl::resolve_name(&index, &name) {
            zti_dsl::NameMatch::Found(id) => {
                zti_dsl::render_symbol_overview(&index, id, depth, max_tokens)
            }
            zti_dsl::NameMatch::Ambiguous(ids) => {
                zti_dsl::search_dep::render_candidates(&index, &ids)
            }
            zti_dsl::NameMatch::NotFound => format!(
                "No symbol named '{name}'. Try projectList / fileTree, or index its \
                 source as a project first."
            ),
        };
        Ok(SearchDepBody { text })
    })
    .await;

    Response::DslSearchDep(result)
}
