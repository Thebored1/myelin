//! Document RAG store: chunk vectors in LanceDB for retrieval. Keeps a whole
//! book out of the model's context — only the top-K matching chunks are pulled
//! in at query time. Its own table/dir, separate from the notes index, so
//! re-indexing notes never wipes ingested documents.
//!
//! Phase E3a: vector search. BM25 / hybrid rerank lands in E3b.
//!
//! This module is a facade over the extracted implementation roots:
//! `types`, `packing`, `schema`, `ingest`, and `search`.

mod ingest;
mod packing;
mod schema;
mod search;
mod types;

#[cfg(test)]
mod tests;

pub use ingest::{contains_document, stored_chunks, upsert_document};
pub use packing::{pack_passages_focused, pack_passages_limited};
pub use search::{search_fts_only, search_hybrid};
pub use types::{DocChunk, RetrievedChunk};
