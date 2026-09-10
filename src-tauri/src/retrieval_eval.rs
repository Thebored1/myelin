//! Reproducible, synthetic baseline measurements for document retrieval.
//!
//! This intentionally runs through the current RAG store in a temporary
//! directory. It never reads or writes application indexes or workspace files.

use crate::rag::{self, DocChunk};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::time::Instant;

#[derive(Debug, Deserialize)]
pub struct RetrievalCorpus {
    pub chunks: Vec<CorpusChunk>,
}

#[derive(Debug, Deserialize)]
pub struct CorpusChunk {
    pub id: String,
    pub source: String,
    pub text: String,
    #[serde(default)]
    pub page: Option<i32>,
    #[serde(default)]
    pub section: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RetrievalQueries {
    pub queries: Vec<EvaluationQuery>,
}

#[derive(Debug, Deserialize)]
pub struct EvaluationQuery {
    pub id: String,
    pub query: String,
    pub relevant_chunk_ids: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct EvaluationReport {
    pub corpus_chunks: usize,
    pub queries: usize,
    pub recall_at_5: f32,
    pub recall_at_10: f32,
    pub mrr_at_10: f32,
    pub ndcg_at_10: f32,
    pub p50_ms: u128,
    pub p95_ms: u128,
    pub cases: Vec<EvaluationCase>,
}

#[derive(Debug, Serialize)]
pub struct EvaluationCase {
    pub id: String,
    pub elapsed_ms: u128,
    pub returned_chunk_ids: Vec<String>,
    pub first_relevant_rank: Option<usize>,
}

pub fn default_fixture_paths() -> (std::path::PathBuf, std::path::PathBuf) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-data/retrieval");
    (root.join("corpus.json"), root.join("queries.json"))
}

pub async fn run_baseline(corpus_path: &Path, queries_path: &Path) -> Result<EvaluationReport> {
    let corpus: RetrievalCorpus = serde_json::from_slice(
        &std::fs::read(corpus_path).with_context(|| format!("read {}", corpus_path.display()))?,
    )
    .with_context(|| format!("parse {}", corpus_path.display()))?;
    let queries: RetrievalQueries = serde_json::from_slice(
        &std::fs::read(queries_path).with_context(|| format!("read {}", queries_path.display()))?,
    )
    .with_context(|| format!("parse {}", queries_path.display()))?;

    let index = tempfile::tempdir().context("create temporary retrieval baseline index")?;
    let chunks = corpus
        .chunks
        .iter()
        .enumerate()
        .map(|(index, chunk)| DocChunk {
            doc_id: chunk.id.clone(),
            source: chunk.source.clone(),
            chunk_index: index as i32,
            text: chunk.text.clone(),
            lexical_text: crate::retrieval_pipeline::lexical_text(
                &chunk.source,
                chunk.section.as_deref(),
                &chunk.text,
            ),
            vector: baseline_embedding(&chunk.text),
            token_count: chunk.text.split_whitespace().count() as i32,
            page_start: chunk.page,
            page_end: chunk.page,
            section_start: chunk.section.clone(),
            section_end: chunk.section.clone(),
            tokenizer_mode: "baseline".into(),
            chunker_version: "retrieval-eval-baseline".into(),
            ..Default::default()
        })
        .collect();
    rag::upsert_document(index.path(), "retrieval-evaluation", chunks)
        .await
        .context("seed temporary retrieval baseline index")?;

    let mut cases = Vec::with_capacity(queries.queries.len());
    let mut recall_at_5 = 0.0_f32;
    let mut recall_at_10 = 0.0_f32;
    let mut reciprocal_rank = 0.0_f32;
    let mut ndcg = 0.0_f32;

    for query in &queries.queries {
        let started = Instant::now();
        let hits = rag::search_hybrid(
            index.path(),
            baseline_embedding(&query.query),
            &query.query,
            10,
            None,
        )
        .await
        .with_context(|| format!("retrieve evaluation query {}", query.id))?;
        let elapsed_ms = started.elapsed().as_millis();
        let returned_chunk_ids: Vec<String> = hits.iter().map(|hit| hit.doc_id.clone()).collect();
        let first_relevant_rank = returned_chunk_ids
            .iter()
            .position(|id| {
                query
                    .relevant_chunk_ids
                    .iter()
                    .any(|relevant| relevant == id)
            })
            .map(|index| index + 1);
        let relevant_at_5 = returned_chunk_ids
            .iter()
            .take(5)
            .filter(|id| {
                query
                    .relevant_chunk_ids
                    .iter()
                    .any(|relevant| relevant == *id)
            })
            .count();
        let relevant_at_10 = returned_chunk_ids
            .iter()
            .take(10)
            .filter(|id| {
                query
                    .relevant_chunk_ids
                    .iter()
                    .any(|relevant| relevant == *id)
            })
            .count();
        let expected = query.relevant_chunk_ids.len().max(1) as f32;
        recall_at_5 += relevant_at_5 as f32 / expected;
        recall_at_10 += relevant_at_10 as f32 / expected;
        if let Some(rank) = first_relevant_rank {
            reciprocal_rank += 1.0 / rank as f32;
        }
        ndcg += normalized_dcg(&returned_chunk_ids, &query.relevant_chunk_ids, 10);
        cases.push(EvaluationCase {
            id: query.id.clone(),
            elapsed_ms,
            returned_chunk_ids,
            first_relevant_rank,
        });
    }

    let denominator = queries.queries.len().max(1) as f32;
    let mut elapsed: Vec<u128> = cases.iter().map(|case| case.elapsed_ms).collect();
    elapsed.sort_unstable();
    Ok(EvaluationReport {
        corpus_chunks: corpus.chunks.len(),
        queries: queries.queries.len(),
        recall_at_5: recall_at_5 / denominator,
        recall_at_10: recall_at_10 / denominator,
        mrr_at_10: reciprocal_rank / denominator,
        ndcg_at_10: ndcg / denominator,
        p50_ms: percentile(&elapsed, 50),
        p95_ms: percentile(&elapsed, 95),
        cases,
    })
}

fn normalized_dcg(returned: &[String], relevant: &[String], limit: usize) -> f32 {
    let dcg = returned
        .iter()
        .take(limit)
        .enumerate()
        .filter(|(_, id)| relevant.iter().any(|expected| expected == *id))
        .map(|(index, _)| 1.0 / ((index + 2) as f32).log2())
        .sum::<f32>();
    let ideal = (0..relevant.len().min(limit))
        .map(|index| 1.0 / ((index + 2) as f32).log2())
        .sum::<f32>();
    if ideal == 0.0 {
        0.0
    } else {
        dcg / ideal
    }
}

fn percentile(sorted: &[u128], percentile: usize) -> u128 {
    if sorted.is_empty() {
        return 0;
    }
    let index = ((sorted.len() - 1) * percentile) / 100;
    sorted[index]
}

/// Mirrors the no-model hashed-vector baseline without exposing workspace
/// indexing internals to this standalone evaluation module.
fn baseline_embedding(text: &str) -> Vec<f32> {
    let mut vector = vec![0.0_f32; crate::embeddings::HASHED_EMBEDDING_DIMENSIONS];
    for token in text
        .to_lowercase()
        .split(|character: char| !character.is_alphanumeric())
        .filter(|token| !token.is_empty())
    {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        token.hash(&mut hasher);
        let hash = hasher.finish();
        let index = (hash as usize) % vector.len();
        vector[index] += if (hash >> 8) & 1 == 0 { 1.0 } else { -1.0 };
    }
    let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
    if norm > 0.0 {
        for value in &mut vector {
            *value /= norm;
        }
    }
    vector
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracked_fixtures_parse() {
        let corpus: RetrievalCorpus =
            serde_json::from_str(include_str!("../test-data/retrieval/corpus.json")).unwrap();
        let queries: RetrievalQueries =
            serde_json::from_str(include_str!("../test-data/retrieval/queries.json")).unwrap();
        assert!(!corpus.chunks.is_empty());
        assert!(!queries.queries.is_empty());
    }

    #[test]
    fn ndcg_rewards_early_relevance() {
        let relevant = vec!["a".to_string(), "b".to_string()];
        assert!(
            normalized_dcg(&["a".into(), "x".into()], &relevant, 10)
                > normalized_dcg(&["x".into(), "a".into()], &relevant, 10)
        );
    }
}
