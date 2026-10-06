//! Retrieval: vector, BM25, hybrid fusion, and adjacent-chunk expansion.

use anyhow::{Context, Result};
use arrow_array::{Array, Float32Array, Int32Array, Int64Array, RecordBatch, StringArray};
use futures_util::TryStreamExt;
use lancedb::query::{ExecutableQuery, QueryBase};
use lancedb::{connect, Table};
use std::path::Path;

use super::packing::materially_overlaps;
use super::schema::open;
use super::types::{RetrievedChunk, RAG_TABLE};

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
    let int64 = |name: &str| {
        batch
            .column_by_name(name)
            .and_then(|c| c.as_any().downcast_ref::<Int64Array>().cloned())
    };
    let int32 = |name: &str| {
        batch
            .column_by_name(name)
            .and_then(|c| c.as_any().downcast_ref::<Int32Array>().cloned())
    };
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
            doc_id: doc_ids
                .as_ref()
                .map(|a| a.value(i).to_string())
                .unwrap_or_default(),
            source: sources
                .as_ref()
                .map(|a| a.value(i).to_string())
                .unwrap_or_default(),
            chunk_index: indices.as_ref().map(|a| a.value(i)).unwrap_or(0),
            text: texts
                .as_ref()
                .map(|a| a.value(i).to_string())
                .unwrap_or_default(),
            distance: dists.as_ref().map(|a| a.value(i)).unwrap_or(0.0),
            bm25_score: scores
                .as_ref()
                .and_then(|a| (!a.is_null(i)).then(|| a.value(i)))
                .filter(|value| value.is_finite()),
            token_count: token_counts.as_ref().map(|a| a.value(i)).unwrap_or(0),
            char_start: char_starts
                .as_ref()
                .and_then(|a| (!a.is_null(i)).then(|| a.value(i))),
            char_end: char_ends
                .as_ref()
                .and_then(|a| (!a.is_null(i)).then(|| a.value(i))),
            page_start: page_starts
                .as_ref()
                .and_then(|a| (!a.is_null(i)).then(|| a.value(i))),
            page_end: page_ends
                .as_ref()
                .and_then(|a| (!a.is_null(i)).then(|| a.value(i))),
            section_start: section_starts
                .as_ref()
                .and_then(|a| (!a.is_null(i)).then(|| a.value(i).to_string())),
            section_end: section_ends
                .as_ref()
                .and_then(|a| (!a.is_null(i)).then(|| a.value(i).to_string())),
        })
        .collect()
}

pub(super) fn doc_filter(doc_ids: Option<&[String]>) -> Option<String> {
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
    let mut stream = query
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

async fn fts_hits(
    table: &Table,
    query_text: &str,
    k: usize,
    doc_ids: Option<&[String]>,
) -> Result<Vec<RetrievedChunk>> {
    let analysis = crate::retrieval_pipeline::RetrievalQuery::analyze_cached(query_text);
    let fts_query = analysis.fts_query();
    if fts_query.is_empty() {
        return Ok(Vec::new());
    }
    let mut query = table.query().full_text_search(
        lancedb::index::scalar::FullTextSearchQuery::new(fts_query)
            .columns(Some(vec!["lexical_text".to_string()])),
    );
    if let Some(filter) = doc_filter(doc_ids) {
        query = query.only_if(filter);
    }
    let mut stream = query.limit(k).execute().await.context("rag fts search")?;
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
        clauses.push(format!(
            "(doc_id = '{doc}' AND chunk_index IN ({before}, {after}))"
        ));
    }
    if clauses.is_empty() {
        return Ok(Vec::new());
    }
    let mut stream = table
        .query()
        .only_if(clauses.join(" OR "))
        .execute()
        .await
        .context("rag adjacent chunk lookup")?;
    let mut chunks = Vec::new();
    while let Some(batch) = stream
        .try_next()
        .await
        .context("rag adjacent chunk stream")?
    {
        chunks.extend(rows_from_batch(&batch));
    }
    Ok(chunks)
}

fn should_expand_neighbors(intent: crate::retrieval_pipeline::RetrievalIntent) -> bool {
    matches!(
        intent,
        crate::retrieval_pipeline::RetrievalIntent::BroadSummary
            | crate::retrieval_pipeline::RetrievalIntent::Quotation
            | crate::retrieval_pipeline::RetrievalIntent::Comparison
            | crate::retrieval_pipeline::RetrievalIntent::Synthesis
    )
}

fn append_neighbors(
    mut seeds: Vec<RetrievedChunk>,
    neighbors: Vec<RetrievedChunk>,
) -> Vec<RetrievedChunk> {
    let mut seen = std::collections::HashSet::new();
    for seed in &seeds {
        seen.insert((seed.doc_id.clone(), seed.chunk_index));
    }
    for neighbor in neighbors {
        if seen.insert((neighbor.doc_id.clone(), neighbor.chunk_index))
            && !materially_overlaps(&neighbor, &seeds)
        {
            seeds.push(neighbor);
        }
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
    let conn = connect(index_dir.to_string_lossy().as_ref())
        .execute()
        .await
        .context("failed to open rag db")?;
    let table = conn
        .open_table(RAG_TABLE)
        .execute()
        .await
        .context("document index is not initialized")?;
    let query = crate::retrieval_pipeline::RetrievalQuery::analyze_cached(query_text);
    let hits = fts_hits(
        &table,
        query_text,
        crate::retrieval_pipeline::FTS_POOL,
        doc_ids,
    )
    .await?;
    Ok(crate::retrieval_pipeline::select_diverse(
        crate::retrieval_pipeline::rank_candidates(&query, Vec::new(), hits, k),
        query.intent,
        doc_ids.is_some_and(|ids| ids.len() == 1),
    )
    .into_iter()
    .map(|candidate| candidate.chunk)
    .collect())
}

/// Vector-only search (kept for tests / when there is no query text).
#[cfg(test)]
pub async fn search(
    index_dir: &Path,
    query_vec: Vec<f32>,
    k: usize,
) -> Result<Vec<RetrievedChunk>> {
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
    )
    .into_iter()
    .map(|candidate| candidate.chunk)
    .collect::<Vec<_>>();
    if should_expand_neighbors(query.intent) {
        match adjacent_hits(&table, &seeds).await {
            Ok(neighbors) => Ok(append_neighbors(seeds, neighbors)),
            Err(error) => {
                log::warn!("[rag] adjacent chunk lookup failed; returning seeds: {error:#}");
                Ok(seeds)
            }
        }
    } else {
        Ok(seeds)
    }
}
