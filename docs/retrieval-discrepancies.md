# Retrieval upgrade discrepancies

## Implemented configuration

- Vector and FTS candidate pools: 24 each; union cap: 36.
- Hybrid fusion: semantic 0.50, normalized BM25 0.25, term coverage 0.15,
  field match 0.10, plus up to 0.12 exact-phrase bonus. FTS-only and
  vector-only modes use the documented renormalized weights.
- Diversity: MMR lambda 0.78, six seeds maximum, two seeds per document for
  ordinary multi-document retrieval and four for broad/comparison requests.
- Adjacent expansion: one preceding and one following chunk for up to three
  seeds, fetched in one LanceDB query for broad, quotation, comparison, and
  synthesis queries.
- RAG schema v4: adds `lexical_text`; FTS indexes this derived field while
  retaining original `text` unchanged.
- Workspace search now uses a derived `note_chunks` table. Chunks preserve
  source text, have nullable semantic vectors for keyword-only operation, and
  are aggregated to one note result with a matching excerpt/section.
- Workspace replacement is staged and published only after Arrow batch/table
  creation succeeds; a previous complete index is retained if staging fails.
- Workspace search inspects live Arrow fields, nullability, fixed vector width,
  and the `lexical_text` FTS index before scanning.
- OCR code and persisted settings remain present for a future bundled runtime,
  but low-text PDF pages do not invoke OCR currently. Native PDF extraction is
  always used; no system Tesseract executable or language pack is required.
- Opening an existing RAG table compares its live Arrow schema against the
  expected columns, nullability, and fixed vector width before use, and now
  inspects/recreates the live `lexical_text` FTS index when missing.
- An optional MS MARCO MiniLM-L6-v2 Q4_K_M reranker has a separate validated
  server, resident-only invocation policy, 12-candidate cap, 800 ms timeout,
  and 0.65 reranker/0.35 base blending.
- Explicit complex agent document searches can request up to three alternate
  queries from an already healthy chat server; malformed or slow plans are discarded.
- Complex-query planning runs concurrently with the first deterministic agent
  retrieval pass, and reranker failures open a three-failure/60-second circuit.

## Live reranker validation

- Tested `/home/paper/Downloads/ms-marco-MiniLM-L6-v2-Q8_0.gguf` with the
  installed `/usr/bin/llama-server` build `b8681-Debian`.
- The GGUF loaded in `--reranking` mode and `/health` returned `{"status":"ok"}`.
- `/v1/models` reported a 512-token context and 384 hidden width. The GGUF
  metadata contains inconsistent display strings referring to “L12”, but the
  loaded architecture reports six BERT blocks and 22.57M parameters, matching
  the expected MiniLM-L6 family.
- `/v1/rerank` returned three unique finite `relevance_score` values and ranked
  “Paris is the capital of France” first over banana and Saturn distractors.
- The temporary server was stopped cleanly after validation. No model file was
  modified. The validated path is now configured in the app's local
  `llama-server.json`; it can be cleared or replaced from settings.

## Changed files and interfaces

- Added `src-tauri/src/retrieval_pipeline.rs`.
- Updated `src-tauri/src/rag.rs`, `src-tauri/src/state/retrieval.rs`,
  `src-tauri/src/ai_turn.rs`, `src-tauri/src/retrieval_eval.rs`, and
  `src-tauri/src/lib.rs`.
- Added `src/lib/extraction/structuralText.ts` and updated the PDF and EPUB
  viewers to preserve more source structure.
- Added `scraper 0.24.0` to the main `src-tauri/Cargo.toml` and main lockfile.
  The backend fetched-page extractor now uses DOM parsing instead of regex tag
  stripping.

## Derived data reset

The following disposable paths were removed after the schema/index upgrade:

- `/home/paper/.local/share/com.paper.myelin/index`
- `/home/paper/.local/share/com.paper.myelin/rag-index`
- `/home/paper/.local/share/com.paper.myelin/query-embeddings.json`
- `/home/paper/.local/share/com.paper.myelin/rag-index-gte-small-384` (legacy
  directory moved to the desktop trash during final validation)

No workspace notes, imported documents, model files, settings, chat history,
or workspace sidecar metadata were deleted.

## Validation results

- `cargo check --manifest-path src-tauri/Cargo.toml --lib`: passed.
- `cargo check --manifest-path src-tauri/Cargo.toml --bin retrieval_eval`:
  passed.
- `cargo test --manifest-path src-tauri/Cargo.toml --lib retrieval_pipeline --jobs 1`:
  3 tests passed.
- `cargo test --manifest-path src-tauri/Cargo.toml --lib rag --jobs 1`:
  9 tests passed.
- `cargo test --manifest-path src-tauri/Cargo.toml --lib ai_turn --jobs 1`:
  21 tests passed.
- `npm run check`: passed with 0 errors and 22 pre-existing Svelte warnings.
- `git diff --check`: passed.

The tracked synthetic evaluator improved from MRR@10 0.7917 / nDCG@10 0.8436
with p50/p95 25/27 ms to MRR@10 1.0 / nDCG@10 1.0 with p50/p95 20/21 ms on
the six-fixture no-model hashed-vector corpus. This is not a live embedding
model benchmark and should not be extrapolated to arbitrary document sets.

## Deviations and unresolved work

- Workspace chunk search still performs a bounded plain LanceDB scan and fuses
  lexical/vector scores in process; it does not yet use concurrent LanceDB
  FTS/vector queries.
- The selected MS MARCO MiniLM-L6-v2 Q4_K_M reranker now has a separate
  validation server, resident-only invocation, `/v1/rerank` validation,
  12-candidate cap, 800 ms timeout, and 0.65/0.35 score blending. A persistent
  three-failure circuit breaker and live model validation remain untested until
  the model is downloaded.
- The local planner now runs only for explicit complex agent document queries,
  only when chat is already healthy, with strict JSON, three-query, 96-token,
  and 600 ms bounds. It has no dedicated latency metric.
- LanceDB 0.14's current FTS construction path was kept at its default
  tokenizer configuration; the planned explicit tokenizer options could not be
  set through the existing API without an unsupported upgrade.
- RAG now compares the live Arrow schema and lists/recreates its lexical FTS
  index when missing. Workspace schema validation still does not compare a
  persisted marker and live table separately.
- MMR uses source-span overlap and token-shingle similarity. Candidate vectors
  are not loaded for pairwise cosine similarity in this pass.
- Neighbor expansion currently applies to broad/quotation/comparison/synthesis
  intent, but does not yet inspect sentence boundaries or merge adjacent source
  spans into a single passage.
- PDF extraction adds a gutter heuristic, row-preserving ` | ` cell separators,
  and repeated edge-line suppression at a 60% page threshold. It still lacks
  dedicated difficult-PDF fixtures and explicit extraction-confidence metadata.
- OCR has dormant `get_ocr_status`, `set_ocr_settings`, and `ocr_pdf_page`
  interfaces. The future bundled implementation still needs TSV execution,
  confidence handling, and timeout enforcement.
- Imported HTML viewer extraction is not yet wired to the new structural DOM
  helper; fetched web pages are.
- EPUB uses structural DOM extraction and no longer truncates chapters, but
  does not resolve navigation/TOC labels beyond the current spine href.
- No live embedding GGUF, reranker GGUF, Tesseract language pack, scanned PDF,
  two-column PDF, or local optional corpus was available for validation.

## Explicitly excluded follow-up work

Bundled OCR execution, full geometry-based table confidence, MMR vector
similarity, adjacent passage merging, and the full evaluation corpus remain
follow-up work. LanceDB was retained and the app version was not changed.
