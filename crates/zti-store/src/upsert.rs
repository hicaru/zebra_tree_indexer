use anyhow::Result;
use arrow::array::{RecordBatch, RecordBatchIterator};
use lancedb::table::Table;

/// Generic upsert: merges `batch` into `table` on the given key column.
///
/// Shared by FilesTable, ProjectsTable, and ChunksTable to avoid
/// duplicating the same merge-insert boilerplate.
pub async fn upsert_batch(table: &Table, key: &str, batch: RecordBatch) -> Result<()> {
    let schema = batch.schema();
    let reader: Box<dyn arrow_array::RecordBatchReader + Send> =
        Box::new(RecordBatchIterator::new(vec![Ok(batch)], schema));

    let mut builder = table.merge_insert(&[key]);
    builder.when_matched_update_all(None);
    builder.when_not_matched_insert_all();
    builder.execute(reader).await?;

    Ok(())
}
