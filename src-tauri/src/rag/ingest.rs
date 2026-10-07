//! Ingest: replace all chunks for a document, then append the new ones.

use anyhow::{Context, Result};
use arrow_array::types::Float32Type;
use arrow_array::{
    Array, ArrayRef, FixedSizeListArray, Float32Array, Int32Array, Int64Array, RecordBatch,
    RecordBatchIterator, StringArray,
};
use lancedb::connect;
use std::path::Path;
use std::sync::Arc;

use super::schema::{open, open_or_create, schema};
use super::types::{DocChunk, DIM, RAG_TABLE};

/// Upper bound on rows read back when diffing a freshly chunked document
/// against what is already stored. Generous enough that no real document hits
/// it; it exists only because the query builder requires an explicit limit.
const STORED_CHUNK_SCAN_LIMIT: usize = 1_000_000;

pub async fn contains_document(index_dir: &Path, doc_id: &str) -> Result<bool> {
    use futures_util::TryStreamExt;
    use lancedb::query::{ExecutableQuery, QueryBase};

    let conn = connect(index_dir.to_string_lossy().as_ref())
        .execute()
        .await
        .context("failed to open rag db")?;
    let table = match conn.open_table(RAG_TABLE).execute().await {
        Ok(table) => table,
        Err(_) => return Ok(false),
    };
    let ids = [doc_id.to_string()];
    let Some(filter) = super::search::doc_filter(Some(&ids)) else {
        return Ok(false);
    };
    let mut stream = table
        .query()
        .only_if(filter)
        .limit(1)
        .execute()
        .await
        .context("failed to check document chunks")?;
    Ok(stream
        .try_next()
        .await
        .context("failed to read document chunk check")?
        .is_some_and(|batch| batch.num_rows() > 0))
}

/// Chunks already stored for a document, keyed by their embed input.
///
/// Re-ingesting a document used to delete every row and re-embed every chunk,
/// so a one-character edit cost a full re-embed of the whole note. Callers diff
/// the freshly chunked text against this map and only embed what actually
/// changed. Keyed by *embed input* rather than stored text, because contextual
/// ingestion prepends a document summary that is not stored on the row — the
/// vector must not be reused if that prefix moved.
#[derive(Default)]
pub struct StoredChunks {
    /// embed input -> stored vector
    pub vectors: std::collections::HashMap<String, Vec<f32>>,
    /// How many chunks the document currently has in the store.
    pub count: usize,
}

pub async fn stored_chunks(index_dir: &Path, doc_id: &str) -> Result<StoredChunks> {
    use futures_util::TryStreamExt;
    use lancedb::query::{ExecutableQuery, QueryBase};

    let conn = match connect(index_dir.to_string_lossy().as_ref())
        .execute()
        .await
    {
        Ok(conn) => conn,
        Err(_) => return Ok(StoredChunks::default()),
    };
    let table = match conn.open_table(RAG_TABLE).execute().await {
        Ok(table) => table,
        Err(_) => return Ok(StoredChunks::default()),
    };
    let Some(filter) = super::search::doc_filter(Some(&[doc_id.to_string()])) else {
        return Ok(StoredChunks::default());
    };
    let mut stream = table
        .query()
        .only_if(filter)
        // `query()` defaults to ten rows. Without an explicit limit this read
        // silently returned only the first ten chunks, so reuse capped out
        // there and a note of any real length re-embedded almost everything on
        // every edit.
        .limit(STORED_CHUNK_SCAN_LIMIT)
        .execute()
        .await
        .context("failed to read stored chunks")?;

    let mut vectors = std::collections::HashMap::new();
    let mut count = 0usize;
    while let Some(batch) = stream
        .try_next()
        .await
        .context("failed to read stored chunks")?
    {
        let texts = batch
            .column_by_name("text")
            .and_then(|c| c.as_any().downcast_ref::<StringArray>().cloned());
        let list = batch
            .column_by_name("vector")
            .and_then(|c| c.as_any().downcast_ref::<FixedSizeListArray>().cloned());
        let (Some(texts), Some(list)) = (texts, list) else {
            continue;
        };
        for row in 0..batch.num_rows() {
            if list.is_null(row) {
                continue;
            }
            // `value` yields an Arc<dyn Array> for this arrow-rs version, so a
            // null row is filtered above rather than unwrapped here.
            let values = list.value(row);
            let vectored: Vec<f32> = values
                .as_any()
                .downcast_ref::<Float32Array>()
                .map(|array| array.values().to_vec())
                .unwrap_or_default();
            vectors.insert(texts.value(row).to_string(), vectored);
            count += 1;
        }
    }
    Ok(StoredChunks { vectors, count })
}

/// Replace all chunks for a document (re-ingest = replace), then append the new
/// ones. The delete is a no-op on first ingest.
pub async fn upsert_document(index_dir: &Path, doc_id: &str, chunks: Vec<DocChunk>) -> Result<()> {
    if chunks.is_empty() {
        let conn = connect(index_dir.to_string_lossy().as_ref())
            .execute()
            .await
            .context("failed to open rag db")?;
        if let Ok(table) = conn.open_table(RAG_TABLE).execute().await {
            let _ = table
                .delete(&format!("doc_id = '{}'", doc_id.replace('\'', "''")))
                .await;
        }
        return Ok(());
    }
    let dimension = chunks
        .first()
        .map(|chunk| chunk.vector.len() as i32)
        .unwrap_or(DIM);
    if dimension <= 0
        || chunks
            .iter()
            .any(|chunk| chunk.vector.len() != dimension as usize)
    {
        anyhow::bail!("RAG document contains inconsistent embedding dimensions");
    }
    let conn = open(index_dir, dimension).await?;
    let table = open_or_create(&conn, dimension).await?;
    let doc_ids = StringArray::from_iter_values(chunks.iter().map(|c| c.doc_id.as_str()));
    let sources = StringArray::from_iter_values(chunks.iter().map(|c| c.source.as_str()));
    let indices = Int32Array::from_iter_values(chunks.iter().map(|c| c.chunk_index));
    let texts = StringArray::from_iter_values(chunks.iter().map(|c| c.text.as_str()));
    let lexical_texts =
        StringArray::from_iter_values(chunks.iter().map(|c| c.lexical_text.as_str()));
    let token_counts = Int32Array::from_iter_values(chunks.iter().map(|c| c.token_count));
    let char_starts = Int64Array::from_iter(chunks.iter().map(|c| c.char_start));
    let char_ends = Int64Array::from_iter(chunks.iter().map(|c| c.char_end));
    let page_starts = Int32Array::from_iter(chunks.iter().map(|c| c.page_start));
    let page_ends = Int32Array::from_iter(chunks.iter().map(|c| c.page_end));
    let section_starts = StringArray::from_iter(chunks.iter().map(|c| c.section_start.as_deref()));
    let section_ends = StringArray::from_iter(chunks.iter().map(|c| c.section_end.as_deref()));
    let tokenizer_modes =
        StringArray::from_iter_values(chunks.iter().map(|c| c.tokenizer_mode.as_str()));
    let chunker_versions =
        StringArray::from_iter_values(chunks.iter().map(|c| c.chunker_version.as_str()));
    let vectors = FixedSizeListArray::from_iter_primitive::<Float32Type, _, _>(
        chunks
            .iter()
            .map(|c| Some(c.vector.iter().copied().map(Some).collect::<Vec<_>>())),
        dimension,
    );
    let s = schema(dimension);
    let batch = RecordBatch::try_new(
        s.clone(),
        vec![
            Arc::new(doc_ids) as ArrayRef,
            Arc::new(sources) as ArrayRef,
            Arc::new(indices) as ArrayRef,
            Arc::new(texts) as ArrayRef,
            Arc::new(lexical_texts) as ArrayRef,
            Arc::new(token_counts) as ArrayRef,
            Arc::new(char_starts) as ArrayRef,
            Arc::new(char_ends) as ArrayRef,
            Arc::new(page_starts) as ArrayRef,
            Arc::new(page_ends) as ArrayRef,
            Arc::new(section_starts) as ArrayRef,
            Arc::new(section_ends) as ArrayRef,
            Arc::new(tokenizer_modes) as ArrayRef,
            Arc::new(chunker_versions) as ArrayRef,
            Arc::new(vectors) as ArrayRef,
        ],
    )?;
    let data = RecordBatchIterator::new(vec![Ok(batch)].into_iter(), s);
    // Construct every Arrow value before touching the current document. This
    // prevents malformed metadata/vectors from deleting a healthy old index.
    let _ = table
        .delete(&format!("doc_id = '{}'", doc_id.replace('\'', "''")))
        .await;
    table
        .add(Box::new(data))
        .execute()
        .await
        .context("failed to append rag chunks")?;

    // Best-effort BM25 full-text index on a derived lexical representation.
    // Ignored if it already exists or the build lacks FTS — vector search still works.
    let _ = table
        .create_index(
            &["lexical_text"],
            lancedb::index::Index::FTS(Default::default()),
        )
        .execute()
        .await;
    Ok(())
}
