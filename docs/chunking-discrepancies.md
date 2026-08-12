# Chunking upgrade discrepancies

## Implemented configuration

- Target: 256 tokens.
- Overlap: approximately 40 tokens.
- Hard maximum: 384 tokens.
- Exact counter: the embedding server's `POST /tokenize` endpoint.
- Fallback counter: UTF-8 byte count, explicitly marked as conservative fallback.

## Intentional deviations

- The RAG row replacement preflights chunk construction and Arrow batch creation
  before deleting old rows. LanceDB 0.14 does not expose a simple atomic rename
  for staged rows, so a storage failure after deletion can still leave that one
  document unindexed. This remains a follow-up atomicity improvement.
- Notebook-specific cell extraction is not yet performed by the RAG chunker.
  Invalid or raw notebook JSON is treated as source-preserving plain text, with
  nullable structural offsets. The viewer path continues to expose notebook
  cells separately.
- The exact tokenizer is queried repeatedly while locating source-preserving
  boundaries. Embedding request batching is intentionally excluded from this
  change and should be addressed as the next performance task.

## Excluded follow-up work

- Configurable embedding dimensions/models.
- Batched embedding requests.
- Reranking, MMR, and adjacent-chunk expansion.
- Fully atomic LanceDB document replacement.

## Files and interfaces changed

- `src-tauri/src/embeddings.rs`: token-aware source-preserving chunker,
  tokenizer abstraction, format hints, and conservative fallback.
- `src-tauri/src/rag.rs`: LanceDB metadata schema, schema marker/reset,
  metadata retrieval, location labels, and overlap suppression.
- `src-tauri/src/state/retrieval.rs`: ingestion wiring, reset event, metadata
  persistence, and chunking metrics.
- `src-tauri/src/lib.rs` and the PDF caller: optional `formatHint` support for
  `ingest_document` and `ensure_document_ingested`.
- Note ingestion manifest versioning now uses the token-aware chunker version.

## Database reset behavior

On the next RAG access, an incompatible or missing `rag-schema.json` causes
only the derived RAG directory to be recreated and the note-ingestion manifest
to be cleared. Notes, PDFs, EPUBs, workspace files, settings, chat history,
model files, and the separate workspace note-search index are not targets.
The current local derived index was not reset during validation.

## Validation

- `cargo check --manifest-path src-tauri/Cargo.toml` — passed (warnings only).
- `cargo test --manifest-path src-tauri/Cargo.toml --lib embeddings::tests` —
  passed: 2 tests.
- `cargo test --manifest-path src-tauri/Cargo.toml --lib rag::tests` — passed.
- `cargo test --manifest-path src-tauri/Cargo.toml --lib ai_turn::tests` —
  passed: 21 tests.
- `npm run check --if-present` — passed with zero errors; 22 existing Svelte
  warnings remain.
- `git diff --check` — passed.

No configured embedding server or representative source corpus was available
in this workspace run, so exact `/tokenize` behavior, manual PDF/Markdown/
LaTeX/notebook ingestion, and old-versus-new latency/chunk-count comparison
remain unmeasured.

## Known unresolved edge cases

- Exact token-count candidate results are not cached within an ingestion.
- The database marker is authoritative for compatibility; this pass does not
  independently inspect legacy LanceDB columns when the marker claims current.
- Notebook JSON is not yet transformed into named cells before chunking.
