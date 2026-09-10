# Embedding runtime and model-contract upgrade discrepancies

## Implemented contract

- Live llama-server probes determine embedding dimensions.
- Known profiles can set context, pooling, and query/document prefixes.
- Nomic Embed Text v1.5 is declared as a 768-dimensional mean-pooled model
  using `search_query: ` and `search_document: ` prefixes.
- Final embedding inputs are token-counted, batched at eight inputs / 2048
  tokens, strictly validated, and L2-normalized.
- RAG and workspace note LanceDB schemas infer their vector width from the
  active complete vector batch.

## Files and interfaces changed

- Embedding profiles, server launch, strict OpenAI-compatible embedding client,
  model validation, RAG storage, note indexing, and Settings model selection.
- `set_embed_model_path` is now asynchronous and returns the validated contract
  rather than an empty result; existing callers can ignore the returned value.

## Intentional deviations and unresolved work

- The app does not yet expose the proposed persistent `EmbeddingModelStatus`
  command or an inline validation-status display. Candidate validation is live
  and blocking, and errors are returned to the existing Settings invocation.
- RAG schema compatibility currently uses the marker's dynamic dimension and
  chunker version; it does not independently inspect a legacy LanceDB table's
  Arrow schema when a marker claims compatibility.
- The workspace note index uses a dynamic vector width but does not yet persist
  a separate workspace embedding-schema marker because the workspace index is
  rebuilt wholesale after a successful model change.
- The long-note centroid implementation uses the established structural
  chunker. It remains dependent on the embedding server tokenizer and does not
  provide a separate no-server semantic centroid mode.
- Validation probes use a temporary loopback port but could race another local
  process between releasing the reservation and starting llama-server.

## Reset behavior

After a successful validated model change, only derived embedding data is
removed: `index`, `rag-index`, the legacy `rag-index-gte-small-384`, and
`query-embeddings.json` under app data. Source files, workspace notes, chats,
settings, and models are not deletion targets.

## Validation

- `cargo check --manifest-path src-tauri/Cargo.toml` — passed (warnings only).
- Final incremental `cargo check --manifest-path src-tauri/Cargo.toml` after
  the empty-document dynamic-index deletion fix — passed (warnings only).
- `cargo test --manifest-path src-tauri/Cargo.toml --lib embeddings::tests` —
  passed: 4 tests.
- `cargo test --manifest-path src-tauri/Cargo.toml --lib model_profiles::tests`
  — passed: 5 tests.
- `cargo test --manifest-path src-tauri/Cargo.toml --lib rag::tests` — passed:
  8 tests.
- `npm run check --if-present` — passed with zero errors; 22 pre-existing
  Svelte warnings remain.
- `git diff --check` — passed.

No embedding GGUF or configured llama embedding runtime was available in this
workspace, so selection-time probe behavior, Nomic's live 768-dimensional
output, batch request reduction, long-note ingestion latency, and model-failure
FTS degradation remain unmeasured.
