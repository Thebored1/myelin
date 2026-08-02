# Current Required Changes

This is the sole working plan for requested repository changes. It contains only outstanding work. Completed, rejected, or obsolete items must be removed rather than retained as history.

## Ground Rules

- Do not edit repository files.
- The only file the planning agent may create or modify is `current-required-change.md`.
- Analyze, review, and propose changes; do not implement them.
- Keep this plan synchronized with the latest request.
- Add newly required work and remove work that is completed, rejected, superseded, or no longer needed.

## LaTeX: Required Change Plan

### Task LATEX-001: Deliver near-real-time LaTeX preview

- Replace the fixed two-second auto-compile delay with a short adaptive debounce, initially targeting 250–400 ms after typing pauses.
- Compile an in-memory editor snapshot rather than waiting for the normal note-save round trip.
- Assign every source snapshot a monotonically increasing revision and apply a returned PDF only when it matches the newest relevant revision and open note.
- Keep at most one Tectonic job active. Coalesce all changes received during that job into one follow-up compile of the newest snapshot.
- Investigate safe cancellation of an obsolete Tectonic session; use revision-based result rejection and coalescing if the embedded engine cannot be interrupted safely.
- Keep the last valid PDF visible while a new compile is pending or while incomplete source temporarily fails.
- Surface unobtrusive states for pending, compiling, current, and error without making the editor feel blocked.
- Avoid setting the application-wide busy state for background preview compilation; manual compilation may still expose an explicit busy state.
- Start background compilation only after the Tectonic support bundle is warmed. First-run bundle acquisition must remain an explicit progress state.
- Instrument edit-to-preview latency separately for debounce, queue wait, TeX compilation, IPC transfer, and PDF rendering.
- Set initial warmed-cache performance goals: median preview update under 500 ms for small documents and under 1 second for typical notes, measured on supported reference hardware.
- Verify rapid typing, edits during compilation, disabling auto-compile, switching notes, closing the preview, and compile errors never display a stale PDF.

### Task LATEX-002: Eliminate package option clashes

- Reproduce the reported failure with a note that requests options for `xcolor`, such as `\usepackage[dvipsnames]{xcolor}`, and capture the exact transformed source sent to Tectonic.
- Treat a source containing preamble commands but no `\documentclass` as a distinct case; do not blindly place its `\usepackage` commands after Myelin's generated `\begin{document}`.
- Split a partial preamble from document body content when safely possible, merge it before `\begin{document}`, and preserve requested package options.
- Replace substring checks in `ensure_packages` with parsing that recognizes active `\usepackage`, `\RequirePackage`, and `\PassOptionsToPackage` declarations.
- Ignore package names appearing only in comments or ordinary document text.
- Support comma-separated package declarations and declarations with options.
- Never load the same package twice with incompatible options. If Myelin supplies a package, combine compatible options before its first load or defer to the user's declaration.
- Prefer leaving a complete document's package choices and ordering untouched. If automatic injection into full documents remains required, restrict it to an explicitly defined safe set and do not inject `xcolor` or `hyperref` without accounting for class and package ordering.
- Return a concise actionable diagnostic when an option clash originates entirely inside the user's own complete preamble; distinguish that from a clash introduced by Myelin's transformation.
- Verify documents using `xcolor` with `dvipsnames`, `table`, multiple options, no options, indirect class loading, and commented-out declarations.
- Acceptance criterion: the reported `Option clash for package xcolor` case compiles without requiring the user to remove valid package options.

### Task LATEX-003: Correct diagnostic line mapping

- Track the exact insertion position and number of generated lines when modifying a full document.
- Subtract injected lines only from diagnostics located after the insertion point.
- Preserve correct mappings for errors before or on `\documentclass`.
- Keep the existing bare-document preamble mapping, but verify its first and last body-line boundaries.
- Add coverage for `l.NN` errors, `on input line NN` errors, duplicates, and diagnostics without line numbers.

### Task LATEX-004: Define and enforce multi-file LaTeX behavior

- Decide whether `.tex` notes should support `\input`, `\include`, local `.sty` files, bibliographies, and relative images.
- If supported, configure a constrained input root based on the note/workspace location and prevent access outside the allowed workspace.
- If unsupported, detect common project-file references and show a clear limitation instead of a generic Tectonic failure.
- Document the selected behavior in the editor or project documentation.

### Task LATEX-005: Clarify compile snapshot semantics

- Decide whether compilation targets the persisted note or the exact editor snapshot at the moment Compile is requested.
- Prefer passing or otherwise pinning an immutable source revision so later saves cannot ambiguously change the requested job.
- Ensure the PDF result is applied only if it corresponds to the relevant note and source revision.
- Avoid showing a completed preview from a note that was closed or replaced during compilation.

### Task LATEX-006: Expand focused LaTeX tests

- Unit-test bare-source wrapping and full-document preservation.
- Test frontmatter removal and empty-document handling.
- Test package detection with comments, options, comma-separated packages, and misleading substrings.
- Add a regression test for partial preambles and `\usepackage[dvipsnames]{xcolor}` in a document without `\documentclass`.
- Test line remapping before and after injected content.
- Test structured error serialization and frontend parsing.
- Test auto-compile changes during an active compile.
- Add integration coverage for first-run cache events, warmed-cache behavior, and failed bundle downloads.
- Add tests for the chosen multi-file policy.

### Task LATEX-007: Update LaTeX documentation

- Explain the real-time preview behavior and what happens while source is temporarily invalid.
- Document first-run support-bundle download and offline behavior after warm-up.
- Document the selected package-injection and multi-file policies.
- State relevant limitations and expected preview latency without promising literally instantaneous compilation.

## Agent, Tool, and Note-Mutation Reliability

### Task AGENT-001: Audit notebook edit argument and execution semantics

- Read `EditNotebookArgs` and `EditNotebookTool` end to end, including the `index`, `content`, and operation fields and their validation.
- Compare the tool implementation with the `stream_chat.rs` deserialization/dispatch pattern and the shared notebook operation functions.
- Define behavior for missing fields, wrong JSON types, negative/out-of-range indexes, unsupported operations, empty content, and edits to cells with outputs.
- Ensure targeted notebook edits cannot escape the selected cell and that successful code-cell edits clear stale outputs consistently.
- Add unit and sidecar integration coverage for valid edits, invalid indexes, malformed arguments, and preservation of unrelated cells.

### Task AGENT-002: Remove destructive format-operation fallbacks

- Replace the `detect_format_op(...).unwrap_or("strip_markdown")` fallback with an explicit refusal when neither the request nor the detected intent identifies a valid operation.
- Never silently convert an ambiguous or malformed request into a destructive formatting operation.
- Return an actionable error that names the accepted format operation values.
- Test invalid explicit operations, ambiguous requests, empty requests, valid detected operations, and every operation in `FORMAT_OPS`.

### Task AGENT-003: Establish the note-write event contract

- Verify whether `AppendNoteTool` emits a full resulting body with `mode: "write"` while other tools emit a delta or append mode.
- Make `strip_prompt_markers` and protocol-residue checks consistent across append, prepend, insert, replace, and write paths before content is cleaned or persisted.
- Define whether `ai://note_written` carries a full authoritative body or an operation-specific delta; document the contract in both Rust and the frontend.
- Make `applyNoteWrite` idempotent for retries and safe when an event arrives after an editor update.
- Add tests for append/prepend/insert/replace, echoed prompt markers, duplicate events, and full-body versus delta payloads.

### Task AGENT-004: Verify approval lifecycle hardening

- Keep exactly one terminal outcome for each approval request: approved, rejected, timed out, cancelled, or channel-closed.
- Ensure pending approval senders are removed on every terminal path and that the frontend does not retain stale approval cards or timers.
- Verify the current timeout/cancellation implementation with deterministic tests rather than relying only on manual timing.
- Test approval during generation, approval after cancellation, timeout followed by a late user response, and app shutdown while approval is pending.

### Task AGENT-005: Complete sidecar cancellation protocol coverage

- Treat the current cancellation route/token work as incomplete until the host, sidecar, upstream generation stream, pending tool wait, and SSE consumer agree on one cancellation lifecycle.
- Confirm protocol-version handling for old sidecars and decide whether cancellation is a required protocol bump or a backward-compatible capability.
- Ensure cancellation emits one final terminal event, preserves already-executed tool results, removes pending tool callbacks, and does not leave a live generation or orphaned request registry entry.
- Add mocked integration tests for cancellation during intent detection, token streaming, tool execution, and SSE disconnect; include a compatibility test for an older sidecar.

## Retrieval and Web-Safety Reliability

### Task REL-001: Preserve and surface RAG failures

- Stop silently converting both vector-search and FTS failures into empty result sets in `search_hybrid`.
- Distinguish a missing/uninitialized table from a real index corruption, schema, embedding-dimension, or query failure.
- Return structured retrieval status to callers so the agent can say that retrieval is unavailable instead of implying that no relevant passages exist.
- Decide whether one backend failing should permit a degraded result from the other backend, and expose that degradation explicitly.
- Add tests for missing tables, FTS-unavailable environments, vector failures, malformed data, and successful hybrid fallback behavior.

### Task REL-002: Close DNS-rebinding and redirect SSRF gaps

- Preserve the existing URL-surface, private-range, timeout, redirect-count, and body-size protections.
- Ensure hostname resolution used for the preflight check is the same resolution used by the actual request, or revalidate every resolved address immediately before each connection and redirect.
- Apply DNS-aware validation to redirect targets, not only literal private IPs and hostname syntax.
- Decide whether redirects should be disabled by default or limited to a small explicit count with per-hop validation.
- Add tests for public hosts resolving to private addresses, DNS changes between validation and request, private redirect targets, IPv4-mapped IPv6, credentials in URLs, and oversized/chunked responses.

## Retrieval and Prompt Efficiency

### Task CACHE-001: Measure the prompt and cache baseline

- Record prompt tokens, cached tokens, newly evaluated tokens, retrieval latency, retrieved-character count, conversation-token count, and time-to-first-token for each turn.
- Add a stable prompt/cache identity to the debug trace based on model, tokenizer/template settings, note identity, tool schema, and prompt revision; never log document contents as part of the identity.
- Distinguish cold-cache, warm-cache, slot-eviction, server-restart, and prefix-mismatch cases.
- Add a repeat-turn benchmark covering the same question, a different question about the same document, a changed document, and an app/model restart.

### Task CACHE-002: Make unchanged-document embeddings durable and explicit

- Verify that document chunks, source text, vectors, embedding model, dimension, chunking parameters, and source revision are persisted together in the document index.
- Compute a content fingerprint so an unchanged document skips re-embedding and re-indexing.
- Invalidate only the affected document when its content, embedding model, dimension, or chunking configuration changes.
- Use atomic manifests and temporary index writes so interrupted ingestion cannot leave vectors associated with the wrong source text.
- Keep the original chunk text alongside each vector; embeddings accelerate retrieval but cannot replace the text supplied to the model.

### Task CACHE-003: Bound and improve passage retrieval

- Establish a default chunk size and overlap, initially targeting roughly 200–400 tokens per chunk.
- Retrieve a small candidate set with hybrid vector/FTS search, then rerank or score candidates before prompt packing.
- Reduce the default final result set from six passages to the smallest useful number, initially 2–3, while preserving neighboring chunks when required for continuity.
- Add a strict evidence token budget, initially targeting 600–1,000 tokens, with overlap and duplicate passage removal.
- Add a broad-question path using a persisted document summary or hierarchical summaries for requests such as “read the attached document,” rather than sending many arbitrary chunks.
- Return retrieval metadata and truncation status so the model can distinguish “limited evidence” from “the document contains no answer.”

### Task CACHE-004: Maximize stable KV-cache prefixes

- Keep the system prompt, unchanged document context, fixed chat tool schemas, and canonical conversation history byte-identical across turns whenever their source inputs are unchanged.
- Keep the current question and retrieved evidence at the end of the prompt so only the dynamic suffix requires evaluation.
- Remove accidental prompt variability from serialization, metadata ordering, duplicated context, and mode/tool-schema changes.
- Verify llama-server `cache_prompt` and stable `id_slot` behavior across repeated turns; log the longest reusable prefix without exposing content.
- Persist and restore slot snapshots only when their identity matches the current model, template, context settings, tool schema, document revision, and application protocol.
- Prewarm the stable document/system prefix after the model is ready, but cancel or invalidate warmups when the note, model, or prompt identity changes.

### Task CACHE-005: Reduce non-document context growth

- Measure how much of the prompt comes from system instructions, tools, document evidence, and conversation history.
- Cap live conversation history by token budget while preserving the current user turn and tool results required for continuity.
- Summarize or compact older completed turns into a stable, explicitly marked summary when history becomes large.
- Avoid including prior assistant claims as authoritative retrieval evidence; use them only to resolve bounded follow-up references.
- Fix the duplicated “Latest question” retrieval-query construction and verify that retrieval context is not duplicated in the model-facing prompt.

### Task CACHE-006: Validate the dual-cache design

- Compare cold and warm time-to-first-token, prompt evaluation time, total prompt tokens, cache reuse ratio, retrieval latency, and answer quality.
- Test unchanged documents, large documents, attached PDFs, broad questions, narrow factual questions, follow-ups, note edits, model restarts, and index rebuilds.
- Acceptance target: unchanged-document repeated turns should reuse the stable prefix substantially; narrow questions should send only a bounded evidence set; retrieval must remain correct when relevant passages are not adjacent.
- Document that embeddings are a persistent semantic-search cache, while KV caching is an exact prompt-computation cache; use both rather than treating one as a replacement for the other.

## Quality, Packaging, and Maintainability

### Task QUALITY-001: Make the validation gates clean

- Run Prettier on the 27 reported files and make `npm run lint` reach and pass ESLint.
- Resolve the 60 Svelte warnings, prioritizing the non-reactive `vditorLoading` state and missing interactive-element roles/labels.
- Remove unused CSS selectors and either split or lazy-load oversized frontend bundles where that materially improves startup.
- Keep `npm run check`, `npm run lint`, `npm run test`, `npm run build`, workspace Rust tests, and sidecar integration tests as documented release gates.

### Task QUALITY-002: Split oversized domain modules

- Extract LaTeX compilation and diagnostics, workspace/file persistence, RAG/indexing, AI turn orchestration, and cache management from `state.rs` behind focused modules.
- Split the large notes route into editor, chat, preview, navigation, and attachment components/stores without changing event contracts.
- Preserve existing public command names and serialized payloads during the split.
- Add module-level tests before moving behavior so the refactor does not hide regressions.

### Task QUALITY-003: Review desktop/webview security and release metadata

- Review the `csp: null` Tauri configuration and define the narrowest CSP compatible with PDF, KaTeX, Pyodide, local assets, and the sidecar workflow.
- Verify Markdown/HTML rendering boundaries remain sanitized when displaying fetched or user-controlled content.
- Reconcile package, Cargo, Tauri, and sidecar version metadata and document the release-version source of truth.
- Add a packaged-app smoke test covering sidecar discovery, resource paths, cache directories, and first-run support-bundle behavior.

## Recommended Order

1. `AGENT-002`: Remove the destructive format fallback.
2. `AGENT-003`: Establish the authoritative note-write event contract.
3. `REL-001`: Stop hiding retrieval failures and define degraded-search behavior.
4. `REL-002`: Close DNS-rebinding and redirect SSRF gaps.
5. `LATEX-002`: Fix the active package-option-clash failure and establish the package-injection policy.
6. `LATEX-004`: Decide and enforce the multi-file policy.
7. `LATEX-005`: Establish source-snapshot and result-validity semantics.
8. `LATEX-001`: Implement near-real-time compilation.
9. `LATEX-003`: Correct diagnostic mapping.
10. `AGENT-001`: Audit notebook edit semantics and add malformed-input coverage.
11. `AGENT-004`: Verify approval timeout/cancellation cleanup.
12. `AGENT-005`: Complete sidecar cancellation protocol and compatibility coverage.
13. `LATEX-006`: Add focused LaTeX unit and integration tests.
14. `QUALITY-001`: Make formatting, type-check, lint, and build gates clean.
15. `LATEX-007`: Update user-facing LaTeX documentation.
16. `QUALITY-002`: Split oversized domain modules.
17. `QUALITY-003`: Complete CSP, rendering, packaging, and release-metadata review.
18. `CACHE-001`: Measure prompt composition and cache reuse before changing retrieval budgets.
19. `CACHE-002`: Verify durable embedding/index fingerprints and safe invalidation.
20. `CACHE-003`: Bound retrieved evidence and add broad-question summaries.
21. `CACHE-004`: Stabilize prompt prefixes and persistent KV-slot identities.
22. `CACHE-005`: Control conversation-history growth and remove duplicated context.
23. `CACHE-006`: Run cold/warm/restart benchmarks and document the dual-cache behavior.

## Planning Note

- Several reliability fixes are already present as uncommitted working-tree changes, including parts of approval timeout cleanup, sidecar cancellation, web-fetch limits, and SSRF checks. Treat the corresponding tasks as verification, completion, and regression-test work unless the implementation is later reverted.
- Do not consider a task complete from code inspection alone: remove it only after its acceptance tests and the relevant validation gates pass.
