//! Document RAG store: chunk vectors in LanceDB for retrieval. Keeps a whole
//! book out of the model's context — only the top-K matching chunks are pulled
//! in at query time. Its own table/dir, separate from the notes index, so
//! re-indexing notes never wipes ingested documents.
//!
//! Phase E3a: vector search. BM25 / hybrid rerank lands in E3b.

use anyhow::{Context, Result};
use arrow_array::types::Float32Type;
use arrow_array::{
    Array, ArrayRef, FixedSizeListArray, Float32Array, Int32Array, Int64Array, RecordBatch,
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
const RAG_SCHEMA_MARKER: &str = "rag-schema.json";
const RAG_SCHEMA_VERSION: u32 = 4;
const LEXICAL_VERSION: &str = "structural-lexical-v1";
/// Fallback width used only by unit tests and no-model compatibility paths.
const DIM: i32 = crate::embeddings::HASHED_EMBEDDING_DIMENSIONS as i32;

/// A chunk to store: which document, where in it, the text, and its embedding.
#[derive(Default)]
pub struct DocChunk {
    pub doc_id: String,
    pub source: String,
    pub chunk_index: i32,
    pub text: String,
    pub lexical_text: String,
    pub vector: Vec<f32>,
    pub token_count: i32,
    pub char_start: Option<i64>,
    pub char_end: Option<i64>,
    pub page_start: Option<i32>,
    pub page_end: Option<i32>,
    pub section_start: Option<String>,
    pub section_end: Option<String>,
    pub tokenizer_mode: String,
    pub chunker_version: String,
}

/// A retrieved chunk with its vector distance (smaller = closer).
#[derive(Debug, Clone, Default)]
pub struct RetrievedChunk {
    pub doc_id: String,
    pub source: String,
    pub chunk_index: i32,
    pub text: String,
    pub distance: f32,
    pub bm25_score: Option<f32>,
    pub token_count: i32,
    pub char_start: Option<i64>,
    pub char_end: Option<i64>,
    pub page_start: Option<i32>,
    pub page_end: Option<i32>,
    pub section_start: Option<String>,
    pub section_end: Option<String>,
}

/// Format ranked passages without allowing retrieval to consume the rest of
/// the model context. The final chunk is truncated when it reaches the budget.
pub fn pack_passages(chunks: Vec<RetrievedChunk>, char_budget: usize) -> String {
    pack_passages_limited(chunks, char_budget, usize::MAX)
}

/// Pack a bounded number of distinct passages for model evidence. Retrieval
/// may return overlapping vector/FTS hits; exact duplicate text should not
/// consume prompt budget twice.
pub fn pack_passages_limited(
    chunks: Vec<RetrievedChunk>,
    char_budget: usize,
    max_chunks: usize,
) -> String {
    let mut used = 0usize;
    let mut packed = 0usize;
    let mut seen = std::collections::HashSet::new();
    let mut kept = Vec::new();
    let mut evidence = String::new();
    for chunk in chunks {
        if used >= char_budget || packed >= max_chunks {
            break;
        }
        let key = chunk.text.trim();
        if key.is_empty() || !seen.insert(key.to_string()) || materially_overlaps(&chunk, &kept) {
            continue;
        }
        let remaining = char_budget - used;
        let excerpt: String = chunk.text.chars().take(remaining).collect();
        if excerpt.trim().is_empty() {
            continue;
        }
        evidence.push_str(&format!("\n\n[{} | document {}]\n{}", chunk_label(&chunk), chunk.doc_id, excerpt.trim()));
        used += excerpt.chars().count();
        packed += 1;
        kept.push(chunk);
    }
    evidence
}

/// Pack a small query-focused excerpt for direct document answers. The normal
/// packer keeps each passage's beginning, which can miss an exact name that
/// appears later in a long chunk. This variant centers the excerpt around the
/// most distinctive query term while retaining the source label.
pub fn pack_passages_focused(
    chunks: Vec<RetrievedChunk>,
    query: &str,
    char_budget: usize,
    max_chunks: usize,
) -> String {
    const STOP: &[&str] = &[
        "about", "does", "from", "mention", "paper", "tell", "that", "this", "what",
        "which", "with",
    ];
    let mut terms: Vec<String> = query
        .to_ascii_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|term| term.len() >= 4 && !STOP.contains(term))
        .map(str::to_string)
        .collect();
    terms.sort_by_key(|term| std::cmp::Reverse(term.len()));

    let mut used = 0usize;
    let mut packed = 0usize;
    let mut seen = std::collections::HashSet::new();
    let mut kept = Vec::new();
    let mut evidence = String::new();
    for chunk in chunks {
        if used >= char_budget || packed >= max_chunks {
            break;
        }
        if chunk.text.trim().is_empty() || !seen.insert(chunk.text.trim().to_string()) || materially_overlaps(&chunk, &kept) {
            continue;
        }
        let remaining = char_budget - used;
        let text_lower = chunk.text.to_ascii_lowercase();
        let match_at = terms
            .iter()
            .find_map(|term| text_lower.find(term));
        let start = match_at
            .map(|at| text_lower[..at].chars().count().saturating_sub(remaining / 3))
            .unwrap_or(0);
        let excerpt: String = chunk
            .text
            .chars()
            .skip(start)
            .take(remaining)
            .collect();
        if excerpt.trim().is_empty() {
            continue;
        }
        evidence.push_str(&format!("\n\n[{} | document {}]\n{}", chunk_label(&chunk), chunk.doc_id, excerpt.trim()));
        used += excerpt.chars().count();
        packed += 1;
        kept.push(chunk);
    }
    evidence
}

fn chunk_label(chunk: &RetrievedChunk) -> String {
    let mut label = chunk.source.clone();
    if let (Some(start), Some(end)) = (chunk.page_start, chunk.page_end) {
        if start == end { label.push_str(&format!(" · page {start}")); }
        else { label.push_str(&format!(" · pages {start}–{end}")); }
    }
    if let (Some(start), Some(end)) = (&chunk.section_start, &chunk.section_end) {
        if start == end { label.push_str(&format!(" · {start}")); }
        else { label.push_str(&format!(" · {start} → {end}")); }
    } else if let Some(section) = &chunk.section_start { label.push_str(&format!(" · {section}")); }
    label
}

fn materially_overlaps(candidate: &RetrievedChunk, kept: &[RetrievedChunk]) -> bool {
    let (Some(start), Some(end)) = (candidate.char_start, candidate.char_end) else { return false; };
    let length = (end - start).max(1) as f32;
    kept.iter().any(|other| {
        if other.doc_id != candidate.doc_id { return false; }
        let (Some(other_start), Some(other_end)) = (other.char_start, other.char_end) else { return false; };
        let overlap = (end.min(other_end) - start.max(other_start)).max(0) as f32;
        overlap / length >= 0.60
    })
}

fn schema(dimension: i32) -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("doc_id", DataType::Utf8, false),
        Field::new("source", DataType::Utf8, false),
        Field::new("chunk_index", DataType::Int32, false),
        Field::new("text", DataType::Utf8, false),
        Field::new("lexical_text", DataType::Utf8, false),
        Field::new("token_count", DataType::Int32, false),
        Field::new("char_start", DataType::Int64, true),
        Field::new("char_end", DataType::Int64, true),
        Field::new("page_start", DataType::Int32, true),
        Field::new("page_end", DataType::Int32, true),
        Field::new("section_start", DataType::Utf8, true),
        Field::new("section_end", DataType::Utf8, true),
        Field::new("tokenizer_mode", DataType::Utf8, false),
        Field::new("chunker_version", DataType::Utf8, false),
        Field::new(
            "vector",
            DataType::FixedSizeList(Arc::new(Field::new("item", DataType::Float32, true)), dimension),
            true,
        ),
    ]))
}

#[derive(serde::Serialize, serde::Deserialize)]
struct SchemaMarker {
    version: u32,
    chunker: String,
    dimension: i32,
    lexical_version: String,
    expected_columns: Vec<String>,
}

/// Ensure an incompatible *derived* RAG index is reset. Workspace files and
/// source documents live elsewhere and are never touched here.
pub fn prepare_schema(index_dir: &Path, dimension: i32) -> Result<bool> {
    let marker_path = index_dir.join(RAG_SCHEMA_MARKER);
    let expected = SchemaMarker {
        version: RAG_SCHEMA_VERSION,
        chunker: crate::embeddings::CHUNKER_VERSION.into(),
        dimension,
        lexical_version: LEXICAL_VERSION.into(),
        expected_columns: schema(dimension).fields().iter().map(|field| field.name().clone()).collect(),
    };
    let current = std::fs::read(&marker_path).ok().and_then(|raw| serde_json::from_slice::<SchemaMarker>(&raw).ok());
    let compatible = current.is_some_and(|marker| {
        marker.version == expected.version
            && marker.chunker == expected.chunker
            && marker.dimension == expected.dimension
            && marker.lexical_version == expected.lexical_version
            && marker.expected_columns == expected.expected_columns
    });
    let reset = index_dir.exists() && !compatible;
    if reset { std::fs::remove_dir_all(index_dir).with_context(|| format!("failed to reset derived rag index {}", index_dir.display()))?; }
    std::fs::create_dir_all(index_dir).with_context(|| format!("failed to create rag index {}", index_dir.display()))?;
    if !compatible {
        let temp = marker_path.with_extension("json.tmp");
        std::fs::write(&temp, serde_json::to_vec_pretty(&expected)?)?;
        std::fs::rename(temp, marker_path)?;
    }
    Ok(reset)
}

async fn open(index_dir: &Path, dimension: i32) -> Result<Connection> {
    prepare_schema(index_dir, dimension)?;
    connect(index_dir.to_string_lossy().as_ref())
        .execute()
        .await
        .context("failed to open rag db")
}

async fn open_or_create(conn: &Connection, dimension: i32) -> Result<Table> {
    let table = match conn.open_table(RAG_TABLE).execute().await {
        Ok(t) => {
            let actual = t.schema().await.context("failed to inspect rag table schema")?;
            let expected = schema(dimension);
            let compatible = actual.fields().iter().zip(expected.fields()).all(|(actual, expected)| actual.name() == expected.name() && actual.data_type() == expected.data_type() && actual.is_nullable() == expected.is_nullable())
                && actual.fields().len() == expected.fields().len();
            if compatible { Ok(t) } else {
                log::warn!("[rag] live schema disagrees with marker; recreating derived document table");
                conn.drop_table(RAG_TABLE).await.context("failed to reset incompatible rag table")?;
                conn.create_empty_table(RAG_TABLE, schema(dimension)).execute().await.context("failed to recreate rag table")
            }
        }
        Err(_) => conn
            .create_empty_table(RAG_TABLE, schema(dimension))
            .execute()
            .await
            .context("failed to create rag table"),
    }?;
    // Marker files cannot prove an FTS index exists. Inspect the live table
    // before returning it and repair a missing lexical index immediately.
    let indices = table.list_indices().await.context("failed to inspect rag indexes")?;
    let has_fts = indices.iter().any(|index| index.columns.iter().any(|column| column == "lexical_text") && matches!(index.index_type, lancedb::index::IndexType::FTS));
    if !has_fts {
        table.create_index(&["lexical_text"], lancedb::index::Index::FTS(Default::default())).execute().await.context("failed to create rag FTS index")?;
    }
    Ok(table)
}

pub async fn contains_document(index_dir: &Path, doc_id: &str) -> Result<bool> {
    let conn = connect(index_dir.to_string_lossy().as_ref()).execute().await.context("failed to open rag db")?;
    let table = match conn.open_table(RAG_TABLE).execute().await {
        Ok(table) => table,
        Err(_) => return Ok(false),
    };
    let ids = [doc_id.to_string()];
    let Some(filter) = doc_filter(Some(&ids)) else {
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

/// Replace all chunks for a document (re-ingest = replace), then append the new
/// ones. The delete is a no-op on first ingest.
pub async fn upsert_document(index_dir: &Path, doc_id: &str, chunks: Vec<DocChunk>) -> Result<()> {
    if chunks.is_empty() {
        let conn = connect(index_dir.to_string_lossy().as_ref()).execute().await.context("failed to open rag db")?;
        if let Ok(table) = conn.open_table(RAG_TABLE).execute().await {
            let _ = table.delete(&format!("doc_id = '{}'", doc_id.replace('\'', "''"))).await;
        }
        return Ok(());
    }
    let dimension = chunks.first().map(|chunk| chunk.vector.len() as i32).unwrap_or(DIM);
    if dimension <= 0 || chunks.iter().any(|chunk| chunk.vector.len() != dimension as usize) { anyhow::bail!("RAG document contains inconsistent embedding dimensions"); }
    let conn = open(index_dir, dimension).await?;
    let table = open_or_create(&conn, dimension).await?;
    let doc_ids = StringArray::from_iter_values(chunks.iter().map(|c| c.doc_id.as_str()));
    let sources = StringArray::from_iter_values(chunks.iter().map(|c| c.source.as_str()));
    let indices = Int32Array::from_iter_values(chunks.iter().map(|c| c.chunk_index));
    let texts = StringArray::from_iter_values(chunks.iter().map(|c| c.text.as_str()));
    let lexical_texts = StringArray::from_iter_values(chunks.iter().map(|c| c.lexical_text.as_str()));
    let token_counts = Int32Array::from_iter_values(chunks.iter().map(|c| c.token_count));
    let char_starts = Int64Array::from_iter(chunks.iter().map(|c| c.char_start));
    let char_ends = Int64Array::from_iter(chunks.iter().map(|c| c.char_end));
    let page_starts = Int32Array::from_iter(chunks.iter().map(|c| c.page_start));
    let page_ends = Int32Array::from_iter(chunks.iter().map(|c| c.page_end));
    let section_starts = StringArray::from_iter(chunks.iter().map(|c| c.section_start.as_deref()));
    let section_ends = StringArray::from_iter(chunks.iter().map(|c| c.section_end.as_deref()));
    let tokenizer_modes = StringArray::from_iter_values(chunks.iter().map(|c| c.tokenizer_mode.as_str()));
    let chunker_versions = StringArray::from_iter_values(chunks.iter().map(|c| c.chunker_version.as_str()));
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
        .create_index(&["lexical_text"], lancedb::index::Index::FTS(Default::default()))
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
    let scores = batch
        .column_by_name("_score")
        .and_then(|c| c.as_any().downcast_ref::<Float32Array>().cloned());
    let indices = batch
        .column_by_name("chunk_index")
        .and_then(|c| c.as_any().downcast_ref::<Int32Array>().cloned());
    let dists = batch
        .column_by_name("_distance")
        .and_then(|c| c.as_any().downcast_ref::<Float32Array>().cloned());
    let int64 = |name: &str| batch.column_by_name(name).and_then(|c| c.as_any().downcast_ref::<Int64Array>().cloned());
    let int32 = |name: &str| batch.column_by_name(name).and_then(|c| c.as_any().downcast_ref::<Int32Array>().cloned());
    let optional_string = |name: &str| str_col(name);
    let token_counts = int32("token_count");
    let char_starts = int64("char_start");
    let char_ends = int64("char_end");
    let page_starts = int32("page_start");
    let page_ends = int32("page_end");
    let section_starts = optional_string("section_start");
    let section_ends = optional_string("section_end");

    (0..batch.num_rows())
        .map(|i| RetrievedChunk {
            doc_id: doc_ids.as_ref().map(|a| a.value(i).to_string()).unwrap_or_default(),
            source: sources.as_ref().map(|a| a.value(i).to_string()).unwrap_or_default(),
            chunk_index: indices.as_ref().map(|a| a.value(i)).unwrap_or(0),
            text: texts.as_ref().map(|a| a.value(i).to_string()).unwrap_or_default(),
            distance: dists.as_ref().map(|a| a.value(i)).unwrap_or(0.0),
            bm25_score: scores.as_ref().and_then(|a| (!a.is_null(i)).then(|| a.value(i))).filter(|value| value.is_finite()),
            token_count: token_counts.as_ref().map(|a| a.value(i)).unwrap_or(0),
            char_start: char_starts.as_ref().and_then(|a| (!a.is_null(i)).then(|| a.value(i))),
            char_end: char_ends.as_ref().and_then(|a| (!a.is_null(i)).then(|| a.value(i))),
            page_start: page_starts.as_ref().and_then(|a| (!a.is_null(i)).then(|| a.value(i))),
            page_end: page_ends.as_ref().and_then(|a| (!a.is_null(i)).then(|| a.value(i))),
            section_start: section_starts.as_ref().and_then(|a| (!a.is_null(i)).then(|| a.value(i).to_string())),
            section_end: section_ends.as_ref().and_then(|a| (!a.is_null(i)).then(|| a.value(i).to_string())),
        })
        .collect()
}

fn doc_filter(doc_ids: Option<&[String]>) -> Option<String> {
    doc_ids.and_then(|ids| {
        let ids: Vec<String> = ids
            .iter()
            .filter(|id| !id.is_empty())
            .map(|id| format!("'{}'", id.replace('\'', "''")))
            .collect();
        (!ids.is_empty()).then(|| format!("doc_id IN ({})", ids.join(", ")))
    })
}

async fn vector_hits(
    table: &Table,
    query_vec: Vec<f32>,
    k: usize,
    doc_ids: Option<&[String]>,
) -> Result<Vec<RetrievedChunk>> {
    let mut query = table
        .query()
        .nearest_to(query_vec)
        .context("rag nearest_to")?;
    query = query.distance_type(lancedb::DistanceType::Cosine);
    if let Some(filter) = doc_filter(doc_ids) {
        query = query.only_if(filter);
    }
    let mut stream = query.limit(k)
        .execute()
        .await
        .context("rag vector search")?;
    let mut out = Vec::new();
    while let Some(batch) = stream.try_next().await.context("rag vector stream")? {
        out.extend(rows_from_batch(&batch));
    }
    Ok(out)
}

async fn fts_hits(
    table: &Table,
    query_text: &str,
    k: usize,
    doc_ids: Option<&[String]>,
) -> Result<Vec<RetrievedChunk>> {
    let analysis = crate::retrieval_pipeline::RetrievalQuery::analyze_cached(query_text);
    let fts_query = analysis.fts_query();
    if fts_query.is_empty() { return Ok(Vec::new()); }
    let mut query = table
        .query()
        .full_text_search(
            lancedb::index::scalar::FullTextSearchQuery::new(fts_query)
                .columns(Some(vec!["lexical_text".to_string()])),
        );
    if let Some(filter) = doc_filter(doc_ids) {
        query = query.only_if(filter);
    }
    let mut stream = query.limit(k)
        .execute()
        .await
        .context("rag fts search")?;
    let mut out = Vec::new();
    while let Some(batch) = stream.try_next().await.context("rag fts stream")? {
        out.extend(rows_from_batch(&batch));
    }
    Ok(out)
}

async fn adjacent_hits(table: &Table, seeds: &[RetrievedChunk]) -> Result<Vec<RetrievedChunk>> {
    let mut clauses = Vec::new();
    for seed in seeds.iter().take(3) {
        let doc = seed.doc_id.replace('\'', "''");
        let before = seed.chunk_index.saturating_sub(1);
        let after = seed.chunk_index.saturating_add(1);
        clauses.push(format!("(doc_id = '{doc}' AND chunk_index IN ({before}, {after}))"));
    }
    if clauses.is_empty() { return Ok(Vec::new()); }
    let mut stream = table.query().only_if(clauses.join(" OR ")).execute().await.context("rag adjacent chunk lookup")?;
    let mut chunks = Vec::new();
    while let Some(batch) = stream.try_next().await.context("rag adjacent chunk stream")? { chunks.extend(rows_from_batch(&batch)); }
    Ok(chunks)
}

fn should_expand_neighbors(intent: crate::retrieval_pipeline::RetrievalIntent) -> bool {
    matches!(intent, crate::retrieval_pipeline::RetrievalIntent::BroadSummary | crate::retrieval_pipeline::RetrievalIntent::Quotation | crate::retrieval_pipeline::RetrievalIntent::Comparison | crate::retrieval_pipeline::RetrievalIntent::Synthesis)
}

fn append_neighbors(mut seeds: Vec<RetrievedChunk>, neighbors: Vec<RetrievedChunk>) -> Vec<RetrievedChunk> {
    let mut seen = std::collections::HashSet::new();
    for seed in &seeds { seen.insert((seed.doc_id.clone(), seed.chunk_index)); }
    for neighbor in neighbors {
        if seen.insert((neighbor.doc_id.clone(), neighbor.chunk_index)) && !materially_overlaps(&neighbor, &seeds) { seeds.push(neighbor); }
    }
    seeds
}

/// Text-only retrieval used when a configured embedding model is unavailable.
pub async fn search_fts_only(
    index_dir: &Path,
    query_text: &str,
    k: usize,
    doc_ids: Option<&[String]>,
) -> Result<Vec<RetrievedChunk>> {
    let conn = connect(index_dir.to_string_lossy().as_ref()).execute().await.context("failed to open rag db")?;
    let table = conn.open_table(RAG_TABLE).execute().await.context("document index is not initialized")?;
    let query = crate::retrieval_pipeline::RetrievalQuery::analyze_cached(query_text);
    let hits = fts_hits(&table, query_text, crate::retrieval_pipeline::FTS_POOL, doc_ids).await?;
    Ok(crate::retrieval_pipeline::select_diverse(
        crate::retrieval_pipeline::rank_candidates(&query, Vec::new(), hits, k),
        query.intent,
        doc_ids.is_some_and(|ids| ids.len() == 1),
    ).into_iter().map(|candidate| candidate.chunk).collect())
}

/// Vector-only search (kept for tests / when there is no query text).
pub async fn search(index_dir: &Path, query_vec: Vec<f32>, k: usize) -> Result<Vec<RetrievedChunk>> {
    let conn = open(index_dir, query_vec.len() as i32).await?;
    let table = match conn.open_table(RAG_TABLE).execute().await {
        Ok(t) => t,
        Err(_) => return Ok(Vec::new()),
    };
    vector_hits(&table, query_vec, k, None).await
}

/// Hybrid search: concurrent cosine-vector and BM25 retrieval, followed by a
/// calibrated deterministic fusion. One failed component degrades to the
/// other; both failing remains an explicit retrieval error.
pub async fn search_hybrid(
    index_dir: &Path,
    query_vec: Vec<f32>,
    query_text: &str,
    k: usize,
    doc_ids: Option<&[String]>,
) -> Result<Vec<RetrievedChunk>> {
    let conn = open(index_dir, query_vec.len() as i32).await?;
    let table = conn
        .open_table(RAG_TABLE)
        .execute()
        .await
        .context("document index is not initialized")?;
    let vector_pool = crate::retrieval_pipeline::VECTOR_POOL;
    let fts_pool = crate::retrieval_pipeline::FTS_POOL;
    // A failed sub-search must not silently read as "no evidence": the callers
    // use an empty result to tell the model the information is not in the store
    // (the EVIDENCE POLICY). Swallowing errors here produced ungrounded answers
    // on a broken index. One failing sub-search is degraded-but-serviceable
    // (fall back to the other); both failing is a retrieval failure.
    let (vector_result, fts_result) = tokio::join!(
        vector_hits(&table, query_vec, vector_pool, doc_ids),
        fts_hits(&table, query_text, fts_pool, doc_ids),
    );
    let (vec_hits, fts) = match (vector_result, fts_result) {
        (Ok(v), Ok(f)) => (v, f),
        (Ok(v), Err(fts_err)) => {
            log::warn!("[rag] FTS search failed; falling back to vector-only: {fts_err:#}");
            (v, Vec::new())
        }
        (Err(vec_err), Ok(f)) => {
            log::warn!("[rag] vector search failed; falling back to FTS-only: {vec_err:#}");
            (Vec::new(), f)
        }
        (Err(vec_err), Err(fts_err)) => {
            return Err(anyhow::anyhow!(
                "document retrieval failed (vector: {vec_err:#}; full-text: {fts_err:#})"
            ));
        }
    };

    let query = crate::retrieval_pipeline::RetrievalQuery::analyze_cached(query_text);
    let seeds = crate::retrieval_pipeline::select_diverse(
        crate::retrieval_pipeline::rank_candidates(&query, vec_hits, fts, k),
        query.intent,
        doc_ids.is_some_and(|ids| ids.len() == 1),
    ).into_iter().map(|candidate| candidate.chunk).collect::<Vec<_>>();
    if should_expand_neighbors(query.intent) {
        match adjacent_hits(&table, &seeds).await {
            Ok(neighbors) => Ok(append_neighbors(seeds, neighbors)),
            Err(error) => { log::warn!("[rag] adjacent chunk lookup failed; returning seeds: {error:#}"); Ok(seeds) }
        }
    } else { Ok(seeds) }
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
            lexical_text: format!("test chunk {idx}"),
            vector: vec![v; DIM as usize],
            ..Default::default()
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
    async fn hybrid_search_reports_uninitialized_index() {
        let dir = tempfile::tempdir().unwrap();
        let err = search_hybrid(dir.path(), vec![0.0; DIM as usize], "query", 5, None)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("document index is not initialized"));
    }

    #[test]
    fn incompatible_marker_resets_only_the_derived_index() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join(RAG_SCHEMA_MARKER);
        std::fs::write(&marker, r#"{"version":1}"#).unwrap();
        std::fs::write(dir.path().join("derived-only"), "stale").unwrap();
        assert!(prepare_schema(dir.path(), DIM).unwrap());
        assert!(!dir.path().join("derived-only").exists());
        let current: SchemaMarker = serde_json::from_slice(&std::fs::read(marker).unwrap()).unwrap();
        assert_eq!(current.version, RAG_SCHEMA_VERSION);
        assert!(current.expected_columns.iter().any(|column| column == "lexical_text"));
    }

    #[test]
    fn limited_passage_packing_deduplicates_and_caps_results() {
        let make = |index: i32, text: &str| RetrievedChunk {
            doc_id: "d".into(),
            source: "source".into(),
            chunk_index: index,
            text: text.into(),
            distance: 0.0,
            ..Default::default()
        };
        let out = pack_passages_limited(
            vec![make(0, "same"), make(1, "same"), make(2, "second")],
            1_000,
            2,
        );
        assert_eq!(out.matches("[source | document d]").count(), 2);
        assert!(out.contains("same"));
        assert!(out.contains("second"));
    }

    #[test]
    fn lexical_analysis_uses_latest_question_not_previous_context() {
        let query = "Latest question: does it hold the works of William Shakespeare\nPrevious assistant context (may be incorrect): prayer blessing curses";
        let analysis = crate::retrieval_pipeline::RetrievalQuery::analyze(query);
        assert!(analysis.original.contains("William Shakespeare"));
        assert!(!analysis.original.contains("prayer"));
        assert!(analysis.fts_query().contains("shakespeare"));
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
                lexical_text: "s the eiffel tower is in paris france".into(),
                vector: vec![0.1; DIM as usize],
                ..Default::default()
            },
            DocChunk {
                doc_id: "d".into(),
                source: "s".into(),
                chunk_index: 1,
                text: "transformers use the attention mechanism".into(),
                lexical_text: "s transformers use the attention mechanism".into(),
                vector: vec![0.9; DIM as usize],
                ..Default::default()
            },
        ];
        upsert_document(dir.path(), "d", docs).await.unwrap();
        // Hybrid: BM25 should surface chunk 1 on the text terms even though the
        // query vector is nearer chunk 0. Just assert the merge runs and returns.
        let res = search_hybrid(dir.path(), vec![0.1; DIM as usize], "attention transformers", 5, None)
            .await
            .unwrap();
        assert!(!res.is_empty());
        assert!(res.iter().any(|c| c.chunk_index == 1));
    }

    #[tokio::test]
    async fn scoped_hybrid_and_document_presence() {
        let dir = tempfile::tempdir().unwrap();
        upsert_document(dir.path(), "note-a", vec![chunk("note-a", 0, 0.5)])
            .await
            .unwrap();
        upsert_document(dir.path(), "note-b", vec![chunk("note-b", 0, 0.5)])
            .await
            .unwrap();

        assert!(contains_document(dir.path(), "note-a").await.unwrap());
        assert!(!contains_document(dir.path(), "missing").await.unwrap());
        let scope = vec!["note-a".to_string()];
        let hits = search_hybrid(
            dir.path(),
            vec![0.5; DIM as usize],
            "chunk",
            10,
            Some(&scope),
        )
        .await
        .unwrap();
        assert!(!hits.is_empty());
        assert!(hits.iter().all(|hit| hit.doc_id == "note-a"));

        let pair = vec!["note-a".to_string(), "note-b".to_string()];
        let pair_hits =
            search_hybrid(dir.path(), vec![0.5; DIM as usize], "chunk", 10, Some(&pair))
                .await
                .unwrap();
        assert!(pair_hits.iter().any(|hit| hit.doc_id == "note-a"));
        assert!(pair_hits.iter().any(|hit| hit.doc_id == "note-b"));
    }

    #[test]
    fn passage_packing_labels_sources_and_respects_adaptive_budget() {
        let chunks = vec![RetrievedChunk {
            doc_id: "pdf-a".into(),
            source: "Paper.pdf".into(),
            chunk_index: 0,
            text: "abcdefghij".into(),
            distance: 1.0,
            ..Default::default()
        }];
        let packed = pack_passages(chunks, 4);
        assert!(packed.contains("[Paper.pdf | document pdf-a]"));
        assert!(packed.ends_with("abcd"));
        assert!(!packed.contains("abcde"));
    }
}
