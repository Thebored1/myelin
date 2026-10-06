//! Shared chunk/retrieval types and store constants for the document RAG index.

pub(super) const RAG_TABLE: &str = "doc_chunks";
pub(super) const RAG_SCHEMA_MARKER: &str = "rag-schema.json";
pub(super) const RAG_SCHEMA_VERSION: u32 = 4;
pub(super) const LEXICAL_VERSION: &str = "structural-lexical-v1";
/// Fallback width used only by unit tests and no-model compatibility paths.
pub(super) const DIM: i32 = crate::embeddings::HASHED_EMBEDDING_DIMENSIONS as i32;

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
