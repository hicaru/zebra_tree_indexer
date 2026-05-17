use zti_protocol::request::ProjectStatusReq;
use zti_protocol::response::{ProjectStatus, Response};

use crate::handlers::with_project;
use crate::state::DaemonState;

pub async fn handle(req: &ProjectStatusReq, state: &DaemonState) -> Response {
    let Some(root) = req.project_root.as_deref() else {
        let reg = state.registry.read().await;
        return Response::ProjectStatus(Ok(ProjectStatus {
            project_root: format!("{} projects loaded", reg.len()),
            total_chunks: 0,
            total_files: 0,
            model_id: state.engine.profile().model_id.clone(),
            model_dim: state.engine.dim() as u32,
            last_indexed_ns: 0,
        }));
    };

    let result = with_project(state, root, |project| async move {
        let dim = state.engine.dim();
        let chunks_len = project.db.chunks_table(dim).await?.len().await.unwrap_or(0);
        let files_len = project.db.files_table().await?.len().await.unwrap_or(0);
        let last_indexed_ns = project
            .db
            .projects_table()
            .await?
            .get(&zti_common::ids::project_id(std::path::Path::new(root)))
            .await
            .ok()
            .flatten()
            .map(|p| p.last_indexed_ns)
            .unwrap_or(0);

        Ok(ProjectStatus {
            project_root: root.to_string(),
            total_chunks: chunks_len as u64,
            total_files: files_len as u64,
            model_id: state.engine.profile().model_id.clone(),
            model_dim: dim as u32,
            last_indexed_ns,
        })
    })
    .await;

    Response::ProjectStatus(result)
}
