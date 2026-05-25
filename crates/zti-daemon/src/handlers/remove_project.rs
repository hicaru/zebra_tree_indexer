use zti_protocol::request::RemoveProjectReq;
use zti_protocol::response::{ErrorBody, Response};

use crate::state::DaemonState;

pub async fn handle(req: &RemoveProjectReq, state: &DaemonState) -> Response {
    let projects = match zti_store::list_projects().await {
        Ok(p) => p,
        Err(e) => {
            return Response::RemoveProject(Err(ErrorBody {
                message: e.to_string(),
            }))
        }
    };

    let row = match projects.iter().find(|p| p.root_path == req.project_root) {
        Some(r) => r,
        None => return Response::RemoveProject(Ok(())),
    };

    let pid: [u8; 32] = match row.project_id.clone().try_into() {
        Ok(p) => p,
        Err(_) => {
            return Response::RemoveProject(Err(ErrorBody {
                message: "invalid project_id length".into(),
            }))
        }
    };

    state.ann.invalidate(&pid).await;

    {
        let mut reg = state.registry.write().await;
        reg.remove(&pid);
    }

    if let Ok(dir) = zti_common::paths::project_dir_path(&pid) {
        let _ = std::fs::remove_dir_all(&dir);
    }

    Response::RemoveProject(Ok(()))
}
