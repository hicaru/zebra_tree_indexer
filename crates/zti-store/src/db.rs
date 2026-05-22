use anyhow::Result;
use lancedb::connect;

use crate::chunks_table::ChunksTable;
use crate::files_table::FilesTable;
use crate::projects_table::{ProjectRow, ProjectsTable};

#[derive(Clone)]
pub struct Db {
    db: lancedb::Connection,
}

impl Db {
    pub async fn open(project_id: &[u8; 32]) -> Result<Self> {
        let root = zti_common::paths::project_dir(project_id)?;
        let lance_dir = root.join("lance");
        std::fs::create_dir_all(&lance_dir)?;

        let db = connect(
            lance_dir
                .to_str()
                .ok_or_else(|| anyhow::anyhow!("invalid path"))?,
        )
        .execute()
        .await?;

        Ok(Self { db })
    }

    pub async fn open_global() -> Result<Self> {
        let data = zti_common::paths::data_dir()?;
        let registry = data.join("registry");
        std::fs::create_dir_all(&registry)?;

        let db = connect(
            registry
                .to_str()
                .ok_or_else(|| anyhow::anyhow!("invalid path"))?,
        )
        .execute()
        .await?;

        Ok(Self { db })
    }

    pub fn connection(&self) -> &lancedb::Connection {
        &self.db
    }

    pub async fn chunks_table(&self, dim: usize) -> Result<ChunksTable> {
        ChunksTable::open(&self.db, dim).await
    }

    pub async fn files_table(&self) -> Result<FilesTable> {
        FilesTable::open(&self.db).await
    }

    pub async fn projects_table(&self) -> Result<ProjectsTable> {
        ProjectsTable::open(&self.db).await
    }
}

pub async fn list_projects() -> Result<Vec<ProjectRow>> {
    let data = zti_common::paths::data_dir()?;
    let projects_dir = data.join("projects");
    if !projects_dir.is_dir() {
        return Ok(Vec::new());
    }

    let dir_entries: Vec<_> = std::fs::read_dir(&projects_dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .collect();

    let mut entries = Vec::with_capacity(dir_entries.len());
    for entry in dir_entries {
        let lance_dir = entry.path().join("lance");
        if !lance_dir.is_dir() {
            continue;
        }

        let db = connect(
            lance_dir
                .to_str()
                .ok_or_else(|| anyhow::anyhow!("invalid path"))?,
        )
        .execute()
        .await?;

        let table_names = db.table_names().execute().await?;
        if !table_names.iter().any(|n| n == "projects") {
            continue;
        }

        let pt = ProjectsTable::open(&db).await?;
        let rows = pt.list().await?;
        entries.extend(rows);
    }

    entries.sort_by(|a, b| a.root_path.cmp(&b.root_path));
    Ok(entries)
}
