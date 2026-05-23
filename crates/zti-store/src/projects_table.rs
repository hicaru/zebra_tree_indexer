use anyhow::{Result, anyhow};
use arrow::array::{
    FixedSizeBinaryArray, RecordBatch, RecordBatchIterator, StringArray, UInt32Array, UInt64Array,
};
use arrow_array::Array;
use futures::StreamExt;
use lancedb::query::ExecutableQuery;
use lancedb::table::Table;
use std::sync::Arc;

use crate::schema;

pub struct ProjectsTable {
    table: Table,
}

impl ProjectsTable {
    pub async fn open(db: &lancedb::Connection) -> Result<Self> {
        let name = "projects";
        let table = if db
            .table_names()
            .execute()
            .await?
            .contains(&name.to_string())
        {
            db.open_table(name).execute().await?
        } else {
            let schema = Arc::new(schema::projects_schema());
            db.create_empty_table(name, schema).execute().await?
        };
        Ok(Self { table })
    }

    pub async fn get(&self, project_id: &[u8; 32]) -> Result<Option<ProjectRow>> {
        let results = self.table.query().execute().await?;
        let mut stream = std::pin::pin!(results);
        while let Some(batch) = stream.next().await {
            let batch = batch?;
            if batch.num_rows() == 0 {
                continue;
            }

            let ids = batch
                .column_by_name("project_id")
                .and_then(|c| c.as_any().downcast_ref::<FixedSizeBinaryArray>())
                .ok_or_else(|| anyhow!("missing/bad column 'project_id'"))?;

            let row = (0..batch.num_rows()).find(|&i| ids.value(i) == project_id.as_slice());
            let Some(i) = row else { continue };

            return Ok(Some(row_from_batch(&batch, i, project_id)));
        }
        Ok(None)
    }

    pub async fn list(&self) -> Result<Vec<ProjectRow>> {
        let total = self.table.count_rows(None).await?;
        let results = self.table.query().execute().await?;
        let mut stream = std::pin::pin!(results);
        let mut rows = Vec::with_capacity(total);
        while let Some(batch) = stream.next().await {
            let batch = batch?;
            if batch.num_rows() == 0 {
                continue;
            }

            let ids = batch
                .column_by_name("project_id")
                .and_then(|c| c.as_any().downcast_ref::<FixedSizeBinaryArray>());

            for i in 0..batch.num_rows() {
                let project_id = ids.map(|a| a.value(i).to_vec()).unwrap_or_default();
                rows.push(row_from_batch(&batch, i, &project_id));
            }
        }
        Ok(rows)
    }

    pub async fn upsert(&self, batch: RecordBatch) -> Result<()> {
        let schema = batch.schema();
        let reader: Box<dyn arrow_array::RecordBatchReader + Send> =
            Box::new(RecordBatchIterator::new(vec![Ok(batch)], schema));

        let mut builder = self.table.merge_insert(&["project_id"]);
        builder.when_matched_update_all(None);
        builder.when_not_matched_insert_all();
        builder.execute(reader).await?;

        Ok(())
    }

    pub async fn len(&self) -> Result<usize> {
        Ok(self.table.count_rows(None).await?)
    }
}

fn row_from_batch(batch: &RecordBatch, i: usize, project_id: &[u8]) -> ProjectRow {
    let root_paths = batch
        .column_by_name("root_path")
        .and_then(|c| c.as_any().downcast_ref::<StringArray>());
    let model_ids = batch
        .column_by_name("model_id")
        .and_then(|c| c.as_any().downcast_ref::<StringArray>());
    let model_dims = batch
        .column_by_name("model_dim")
        .and_then(|c| c.as_any().downcast_ref::<UInt32Array>());
    let total_chunks = batch
        .column_by_name("total_chunks")
        .and_then(|c| c.as_any().downcast_ref::<UInt64Array>());
    let total_files = batch
        .column_by_name("total_files")
        .and_then(|c| c.as_any().downcast_ref::<UInt64Array>());
    let last_indexed = batch
        .column_by_name("last_indexed_ns")
        .and_then(|c| c.as_any().downcast_ref::<UInt64Array>());
    let created_at = batch
        .column_by_name("created_at_ns")
        .and_then(|c| c.as_any().downcast_ref::<UInt64Array>());
    let search_method = batch
        .column_by_name("search_method")
        .and_then(|c| c.as_any().downcast_ref::<StringArray>());
    let search_params = batch
        .column_by_name("search_params")
        .and_then(|c| c.as_any().downcast_ref::<StringArray>());

    ProjectRow {
        project_id: project_id.to_vec(),
        root_path: root_paths
            .map(|a| a.value(i).to_string())
            .unwrap_or_default(),
        model_id: model_ids
            .map(|a| a.value(i).to_string())
            .unwrap_or_default(),
        model_dim: model_dims.map(|a| a.value(i)).unwrap_or(0),
        total_chunks: total_chunks.map(|a| a.value(i)).unwrap_or(0),
        total_files: total_files.map(|a| a.value(i)).unwrap_or(0),
        last_indexed_ns: last_indexed.map(|a| a.value(i)).unwrap_or(0),
        created_at_ns: created_at.map(|a| a.value(i)).unwrap_or(0),
        search_method: search_method.and_then(|a| {
            if a.is_null(i) {
                None
            } else {
                Some(a.value(i).to_string())
            }
        }),
        search_params: search_params.and_then(|a| {
            if a.is_null(i) {
                None
            } else {
                Some(a.value(i).to_string())
            }
        }),
    }
}

#[derive(Debug, Clone)]
pub struct ProjectRow {
    pub project_id: Vec<u8>,
    pub root_path: String,
    pub model_id: String,
    pub model_dim: u32,
    pub total_chunks: u64,
    pub total_files: u64,
    pub last_indexed_ns: u64,
    pub created_at_ns: u64,
    pub search_method: Option<String>,
    pub search_params: Option<String>,
}
