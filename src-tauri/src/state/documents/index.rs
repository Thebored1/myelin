// LanceDB index rebuild for the workspace note store.

use super::*;
pub(crate) use anyhow::{anyhow, Context, Result};
pub(crate) use arrow_array::types::Float32Type;
pub(crate) use arrow_array::{
    ArrayRef, FixedSizeListArray, Int32Array, Int64Array, RecordBatch, RecordBatchIterator,
    StringArray,
};
pub(crate) use arrow_schema::{DataType, Field, Schema};
pub(crate) use lancedb::connection::Connection;
pub(crate) use lancedb::{connect, Table};
pub(crate) use std::ffi::OsStr;
pub(crate) use std::fs;
pub(crate) use std::path::Path;
pub(crate) use std::sync::Arc;


pub(crate) async fn rebuild_lancedb(
    index_dir: &Path,
    chunks: &[WorkspaceNoteChunk],
) -> Result<Table> {
    let parent = index_dir
        .parent()
        .ok_or_else(|| anyhow!("workspace index has no parent directory"))?;
    fs::create_dir_all(parent)?;
    let name = index_dir
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or("index");
    let staging_dir = parent.join(format!(".{name}-staging"));
    let previous_dir = parent.join(format!(".{name}-previous"));
    if staging_dir.exists() {
        fs::remove_dir_all(&staging_dir)
            .context("failed to clear stale workspace index staging directory")?;
    }
    if previous_dir.exists() {
        fs::remove_dir_all(&previous_dir)
            .context("failed to clear stale workspace index backup")?;
    }
    fs::create_dir_all(&staging_dir)
        .context("failed to create workspace index staging directory")?;
    let dimension = chunks
        .iter()
        .find_map(|chunk| chunk.vector.as_ref().map(|v| v.len() as i32))
        .unwrap_or(EMBEDDING_DIM);
    if dimension <= 0
        || chunks
            .iter()
            .filter_map(|chunk| chunk.vector.as_ref())
            .any(|vector| {
                vector.len() != dimension as usize || vector.iter().any(|value| !value.is_finite())
            })
    {
        anyhow::bail!("workspace note index contains inconsistent embedding dimensions");
    }
    let connection = open_database(&staging_dir).await?;
    let schema = Arc::new(Schema::new(vec![
        Field::new("note_id", DataType::Utf8, false),
        Field::new("title", DataType::Utf8, false),
        Field::new("tags_text", DataType::Utf8, false),
        Field::new("path", DataType::Utf8, false),
        Field::new("updated_at", DataType::Utf8, false),
        Field::new("chunk_index", DataType::Int32, false),
        Field::new("text", DataType::Utf8, false),
        Field::new("lexical_text", DataType::Utf8, false),
        Field::new("token_count", DataType::Int32, false),
        Field::new("char_start", DataType::Int64, true),
        Field::new("char_end", DataType::Int64, true),
        Field::new("section_start", DataType::Utf8, true),
        Field::new("section_end", DataType::Utf8, true),
        Field::new("embedding_fingerprint", DataType::Utf8, false),
        Field::new(
            "vector",
            DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, true)),
                dimension,
            ),
            true,
        ),
    ]));
    if chunks.is_empty() {
        let table = connection
            .create_empty_table(TABLE_NAME, schema)
            .execute()
            .await
            .context("failed to create empty lancedb table");
        let _ = table?;
        publish_workspace_index(index_dir, &staging_dir, &previous_dir)?;
        return open_database(index_dir)
            .await?
            .open_table(TABLE_NAME)
            .execute()
            .await
            .context("failed to reopen published workspace index");
    }
    let ids = StringArray::from_iter_values(chunks.iter().map(|chunk| chunk.note_id.as_str()));
    let titles = StringArray::from_iter_values(chunks.iter().map(|chunk| chunk.title.as_str()));
    let tags = StringArray::from_iter_values(chunks.iter().map(|chunk| chunk.tags_text.as_str()));
    let paths = StringArray::from_iter_values(chunks.iter().map(|chunk| chunk.path.as_str()));
    let updated_at =
        StringArray::from_iter_values(chunks.iter().map(|chunk| chunk.updated_at.as_str()));
    let chunk_indices = Int32Array::from_iter_values(chunks.iter().map(|chunk| chunk.chunk_index));
    let texts = StringArray::from_iter_values(chunks.iter().map(|chunk| chunk.text.as_str()));
    let lexical =
        StringArray::from_iter_values(chunks.iter().map(|chunk| chunk.lexical_text.as_str()));
    let counts = Int32Array::from_iter_values(chunks.iter().map(|chunk| chunk.token_count));
    let starts = Int64Array::from_iter(chunks.iter().map(|chunk| chunk.char_start));
    let ends = Int64Array::from_iter(chunks.iter().map(|chunk| chunk.char_end));
    let section_starts =
        StringArray::from_iter(chunks.iter().map(|chunk| chunk.section_start.as_deref()));
    let section_ends =
        StringArray::from_iter(chunks.iter().map(|chunk| chunk.section_end.as_deref()));
    let fingerprints = StringArray::from_iter_values(
        chunks
            .iter()
            .map(|chunk| chunk.embedding_fingerprint.as_str()),
    );
    let vectors = FixedSizeListArray::from_iter_primitive::<Float32Type, _, _>(
        chunks.iter().map(|chunk| {
            chunk
                .vector
                .as_ref()
                .map(|vector| vector.iter().copied().map(Some).collect::<Vec<_>>())
        }),
        dimension,
    );
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(ids) as ArrayRef,
            Arc::new(titles) as ArrayRef,
            Arc::new(tags) as ArrayRef,
            Arc::new(paths) as ArrayRef,
            Arc::new(updated_at) as ArrayRef,
            Arc::new(chunk_indices) as ArrayRef,
            Arc::new(texts) as ArrayRef,
            Arc::new(lexical) as ArrayRef,
            Arc::new(counts) as ArrayRef,
            Arc::new(starts) as ArrayRef,
            Arc::new(ends) as ArrayRef,
            Arc::new(section_starts) as ArrayRef,
            Arc::new(section_ends) as ArrayRef,
            Arc::new(fingerprints) as ArrayRef,
            Arc::new(vectors) as ArrayRef,
        ],
    )?;
    let data = RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema);
    let table = connection
        .create_table(TABLE_NAME, Box::new(data))
        .execute()
        .await
        .context("failed to create lancedb table")?;
    table
        .create_index(
            &["lexical_text"],
            lancedb::index::Index::FTS(Default::default()),
        )
        .execute()
        .await
        .context("failed to create workspace FTS index")?;
    publish_workspace_index(index_dir, &staging_dir, &previous_dir)?;
    open_database(index_dir)
        .await?
        .open_table(TABLE_NAME)
        .execute()
        .await
        .context("failed to reopen published workspace index")
}
fn publish_workspace_index(
    index_dir: &Path,
    staging_dir: &Path,
    previous_dir: &Path,
) -> Result<()> {
    if index_dir.exists() {
        fs::rename(index_dir, previous_dir).context("failed to stage previous workspace index")?;
    }
    if let Err(error) = fs::rename(staging_dir, index_dir) {
        if previous_dir.exists() {
            let _ = fs::rename(previous_dir, index_dir);
        }
        return Err(error).context("failed to publish workspace index");
    }
    if previous_dir.exists() {
        fs::remove_dir_all(previous_dir).context("failed to remove previous workspace index")?;
    }
    Ok(())
}
pub(crate) async fn open_database(index_dir: &Path) -> Result<Connection> {
    connect(index_dir.to_string_lossy().as_ref())
        .execute()
        .await
        .context("failed to open lancedb")
}
