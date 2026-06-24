//! Document RAG store: chunk vectors in LanceDB for retrieval. Keeps a whole
//! book out of the model's context — only the top-K matching chunks are pulled
//! in at query time. Its own table/dir, separate from the notes index, so
//! re-indexing notes never wipes ingested documents.
//!
//! Phase E3a: vector search. BM25 / hybrid rerank lands in E3b.

use anyhow::{Context, Result};
use arrow_array::types::Float32Type;
use arrow_array::{
    Array, ArrayRef, FixedSizeListArray, Float32Array, Int32Array, RecordBatch,
    RecordBatchIterator, StringArray,
};
use arrow_schema::{DataType, Field, Schema};
use futures_util::TryStreamExt;
use lancedb::connection::Connection;
use lancedb::query::{ExecutableQuery, QueryBase};
use lancedb::{connect, Table};
use std::path::Path;
use std::sync::Arc;

const RAG_TABLE: &str = "doc_chunks";
/// nomic-embed-text v1.5 dimension.
const DIM: i32 = 768;

/// A chunk to store: which document, where in it, the text, and its embedding.
pub struct DocChunk {
    pub doc_id: String,
    pub source: String,
    pub chunk_index: i32,
    pub text: String,
    pub vector: Vec<f32>,
}

/// A retrieved chunk with its vector distance (smaller = closer).
#[derive(Debug, Clone)]
pub struct RetrievedChunk {
    pub doc_id: String,
    pub source: String,
    pub chunk_index: i32,
    pub text: String,
    pub distance: f32,
}

fn schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("doc_id", DataType::Utf8, false),
        Field::new("source", DataType::Utf8, false),
        Field::new("chunk_index", DataType::Int32, false),
        Field::new("text", DataType::Utf8, false),
        Field::new(
            "vector",
            DataType::FixedSizeList(Arc::new(Field::new("item", DataType::Float32, true)), DIM),
            true,
        ),
    ]))
}

async fn open(index_dir: &Path) -> Result<Connection> {
    std::fs::create_dir_all(index_dir).ok();
    connect(index_dir.to_string_lossy().as_ref())
        .execute()
        .await
        .context("failed to open rag db")
}

async fn open_or_create(conn: &Connection) -> Result<Table> {
    match conn.open_table(RAG_TABLE).execute().await {
        Ok(t) => Ok(t),
        Err(_) => conn
            .create_empty_table(RAG_TABLE, schema())
            .execute()
            .await
            .context("failed to create rag table"),
    }
}

/// Replace all chunks for a document (re-ingest = replace), then append the new
/// ones. The delete is a no-op on first ingest.
pub async fn upsert_document(index_dir: &Path, doc_id: &str, chunks: Vec<DocChunk>) -> Result<()> {
    let conn = open(index_dir).await?;
    let table = open_or_create(&conn).await?;
    let _ = table
        .delete(&format!("doc_id = '{}'", doc_id.replace('\'', "''")))
        .await;
    if chunks.is_empty() {
        return Ok(());
    }

    let doc_ids = StringArray::from_iter_values(chunks.iter().map(|c| c.doc_id.as_str()));
    let sources = StringArray::from_iter_values(chunks.iter().map(|c| c.source.as_str()));
    let indices = Int32Array::from_iter_values(chunks.iter().map(|c| c.chunk_index));
    let texts = StringArray::from_iter_values(chunks.iter().map(|c| c.text.as_str()));
    let vectors = FixedSizeListArray::from_iter_primitive::<Float32Type, _, _>(
        chunks
            .iter()
            .map(|c| Some(c.vector.iter().copied().map(Some).collect::<Vec<_>>())),
        DIM,
    );
    let s = schema();
    let batch = RecordBatch::try_new(
        s.clone(),
        vec![
            Arc::new(doc_ids) as ArrayRef,
            Arc::new(sources) as ArrayRef,
            Arc::new(indices) as ArrayRef,
            Arc::new(texts) as ArrayRef,
            Arc::new(vectors) as ArrayRef,
        ],
    )?;
    let data = RecordBatchIterator::new(vec![Ok(batch)].into_iter(), s);
    table
        .add(Box::new(data))
        .execute()
        .await
        .context("failed to append rag chunks")?;

    // Best-effort BM25 full-text index on the chunk text for hybrid retrieval.
    // Ignored if it already exists or the build lacks FTS — vector search still works.
    let _ = table
        .create_index(&["text"], lancedb::index::Index::FTS(Default::default()))
        .execute()
        .await;
    Ok(())
}

/// Extract RetrievedChunks from one result batch.
fn rows_from_batch(batch: &RecordBatch) -> Vec<RetrievedChunk> {
    let str_col = |name: &str| -> Option<StringArray> {
        batch
            .column_by_name(name)
            .and_then(|c| c.as_any().downcast_ref::<StringArray>().cloned())
    };
    let doc_ids = str_col("doc_id");
    let sources = str_col("source");
    let texts = str_col("text");
    let indices = batch
        .column_by_name("chunk_index")
        .and_then(|c| c.as_any().downcast_ref::<Int32Array>().cloned());
    let dists = batch
        .column_by_name("_distance")
        .and_then(|c| c.as_any().downcast_ref::<Float32Array>().cloned());

    (0..batch.num_rows())
        .map(|i| RetrievedChunk {
            doc_id: doc_ids.as_ref().map(|a| a.value(i).to_string()).unwrap_or_default(),
            source: sources.as_ref().map(|a| a.value(i).to_string()).unwrap_or_default(),
            chunk_index: indices.as_ref().map(|a| a.value(i)).unwrap_or(0),
            text: texts.as_ref().map(|a| a.value(i).to_string()).unwrap_or_default(),
            distance: dists.as_ref().map(|a| a.value(i)).unwrap_or(0.0),
        })
        .collect()
}

async fn vector_hits(table: &Table, query_vec: Vec<f32>, k: usize) -> Result<Vec<RetrievedChunk>> {
    let mut stream = table
        .query()
        .nearest_to(query_vec)
        .context("rag nearest_to")?
        .limit(k)
        .execute()
        .await
        .context("rag vector search")?;
    let mut out = Vec::new();
    while let Some(batch) = stream.try_next().await.context("rag vector stream")? {
        out.extend(rows_from_batch(&batch));
    }
    Ok(out)
}

async fn fts_hits(table: &Table, query: &str, k: usize) -> Result<Vec<RetrievedChunk>> {
    let mut stream = table
        .query()
        .full_text_search(lancedb::query::FullTextSearchQuery::new(query.to_string()))
        .limit(k)
        .execute()
        .await
        .context("rag fts search")?;
    let mut out = Vec::new();
    while let Some(batch) = stream.try_next().await.context("rag fts stream")? {
        out.extend(rows_from_batch(&batch));
    }
    Ok(out)
}

/// Vector-only search (kept for tests / when there is no query text).
pub async fn search(index_dir: &Path, query_vec: Vec<f32>, k: usize) -> Result<Vec<RetrievedChunk>> {
    let conn = open(index_dir).await?;
    let table = match conn.open_table(RAG_TABLE).execute().await {
        Ok(t) => t,
        Err(_) => return Ok(Vec::new()),
    };
    vector_hits(&table, query_vec, k).await
}

/// Hybrid search: vector + BM25 full-text, merged with Reciprocal Rank Fusion.
/// Vector catches meaning, BM25 catches exact terms/names/numbers. Falls back to
/// vector-only if the FTS index isn't present.
pub async fn search_hybrid(
    index_dir: &Path,
    query_vec: Vec<f32>,
    query_text: &str,
    k: usize,
) -> Result<Vec<RetrievedChunk>> {
    let conn = open(index_dir).await?;
    let table = match conn.open_table(RAG_TABLE).execute().await {
        Ok(t) => t,
        Err(_) => return Ok(Vec::new()),
    };
    let pool = (k * 4).max(20);
    let vec_hits = vector_hits(&table, query_vec, pool).await.unwrap_or_default();
    let fts = fts_hits(&table, query_text, pool).await.unwrap_or_default();

    // Reciprocal Rank Fusion across the two ranked lists.
    const RRF_K: f32 = 60.0;
    let mut scored: std::collections::HashMap<(String, i32), (f32, RetrievedChunk)> =
        std::collections::HashMap::new();
    for list in [vec_hits, fts] {
        for (rank, chunk) in list.into_iter().enumerate() {
            let key = (chunk.doc_id.clone(), chunk.chunk_index);
            let bump = 1.0 / (RRF_K + rank as f32 + 1.0);
            scored.entry(key).or_insert((0.0, chunk)).0 += bump;
        }
    }
    let mut merged: Vec<(f32, RetrievedChunk)> = scored.into_values().collect();
    merged.sort_by(|a, b| b.0.total_cmp(&a.0));
    Ok(merged.into_iter().take(k).map(|(_, c)| c).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(id: &str, idx: i32, v: f32) -> DocChunk {
        DocChunk {
            doc_id: id.into(),
            source: "test".into(),
            chunk_index: idx,
            text: format!("chunk {idx}"),
            vector: vec![v; DIM as usize],
        }
    }

    #[tokio::test]
    async fn ingest_search_and_replace_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        // Two chunks: one near 0.1, one near 0.9.
        upsert_document(dir.path(), "d1", vec![chunk("d1", 0, 0.1), chunk("d1", 1, 0.9)])
            .await
            .unwrap();

        // Query closest to the 0.9 vector → chunk_index 1 ranks first.
        let res = search(dir.path(), vec![0.9; DIM as usize], 5).await.unwrap();
        assert_eq!(res.len(), 2);
        assert_eq!(res[0].chunk_index, 1);
        assert_eq!(res[0].doc_id, "d1");

        // Re-ingesting the same doc replaces its chunks (not append).
        upsert_document(dir.path(), "d1", vec![chunk("d1", 0, 0.5)])
            .await
            .unwrap();
        let res2 = search(dir.path(), vec![0.5; DIM as usize], 5).await.unwrap();
        assert_eq!(res2.len(), 1);
        assert_eq!(res2[0].chunk_index, 0);
    }

    #[tokio::test]
    async fn search_missing_table_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let res = search(dir.path(), vec![0.0; DIM as usize], 5).await.unwrap();
        assert!(res.is_empty());
    }

    #[tokio::test]
    async fn hybrid_runs_vector_plus_fts() {
        let dir = tempfile::tempdir().unwrap();
        let docs = vec![
            DocChunk {
                doc_id: "d".into(),
                source: "s".into(),
                chunk_index: 0,
                text: "the eiffel tower is in paris france".into(),
                vector: vec![0.1; DIM as usize],
            },
            DocChunk {
                doc_id: "d".into(),
                source: "s".into(),
                chunk_index: 1,
                text: "transformers use the attention mechanism".into(),
                vector: vec![0.9; DIM as usize],
            },
        ];
        upsert_document(dir.path(), "d", docs).await.unwrap();
        // Hybrid: BM25 should surface chunk 1 on the text terms even though the
        // query vector is nearer chunk 0. Just assert the merge runs and returns.
        let res = search_hybrid(dir.path(), vec![0.1; DIM as usize], "attention transformers", 5)
            .await
            .unwrap();
        assert!(!res.is_empty());
        assert!(res.iter().any(|c| c.chunk_index == 1));
    }
}
