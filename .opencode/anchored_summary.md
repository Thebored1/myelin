## Objective

- Implement **section-scoped KV caching** for massive documents: instead of whole-doc context or per-query RAG evidence, the model's context is the section currently visible in the split-pane viewer. Sections are cached so the doc "takes no context" beyond the active section, with near-zero time-to-first-token on every switch and across restarts.

## Decided Approach

- **Active-section context:** the split-pane viewer reports `active_section`; `ask_ai_stream` builds the system prefix from that section (query-independent → byte-identical prefix → `cache_prompt` always hits). Reuse existing per-type sectioning (`outline()` in note_prompt.rs: md headings, `\section`, ipynb cells) + `embeddings.rs` chunker as the section source.
- **Persist section KV to disk (llama.cpp already supports this):**
  - Launch `llama-server` with `--slot-save-path <dir>` (new flag in `llama_server.rs`).
  - `POST /slots/{id_slot}?action=save|restore|erase` — save each section's prompt cache once (clean state, no history), restore on page-turn.
  - Section tokens precomputed at ingest (free — tokenization costs no context); KV snapshot eval'd once per section, then restored forever.
- **Default: single slot + disk LRU** of section KV files. **Optional tier: multi-slot** (`--parallel`, `id_slot` pinning) for RAM-rich machines — fastest switch, costs one ctx of KV per slot.
- **History = Option B (single canonical conversation — decided).** History is NOT stored per-section. Myelin's per-note system-less conversation stays the source of truth; files hold only CLEAN section prefixes. After restore, Myelin appends at most one sanitized, relevant prior question/answer pair (1,200 characters total) from anywhere in that universal history; pronoun follow-ups fall back to the latest pair and unrelated questions replay none. Option A (per-section conversation checkpoints) is rejected and removed because it branches history, creates large files, and makes restore nondeterministic.
- **Per-note session checkpoint (optional resume path):** a single snapshot may later resume the exact same active section and conversation. It must never create one conversation checkpoint per section.
- **RAG stays only for explicit wider scope:** whole-document requests and numbered pages different from the visible page. Ordinary viewer chat defaults to the active section and does not pay embedding/RAG latency.
- **Eager whole-doc pre-cache (implemented):** each viewer emits every section. `cache_note_sections` evaluates each clean prefix once and saves it with llama-server's positive `n_saved` token count. “Prepared” is shown only if every save succeeds; partial failures remain visible. A dedicated slot mutex serializes background prime/save/restore with user chat and keeps the resident-page marker accurate.
- **Wire parity:** section v4 asks llama-server `/apply-template` to render two sentinel user turns that differ at the first content character, takes their exact common prefix immediately before dynamic user content, and evaluates that raw prefix through `/completion` with `n_predict: 0`. No sentinel text or dummy user/completion suffix is saved. Managed non-thinking models use `--reasoning off`; the wire format is versioned so incompatible snapshots rebuild automatically.

## Expected Numbers (measured baselines)

- Perfect-reuse TTFT today: ~0.6s (user trace: 3982 prompt / 3978 cached, 99.9%).
- Cold section eval: ~40s CPU (LFM2-8B, 95 prompt tok/s) / ~2-6s GPU for a ~4k-token section.
- Restore: ~0.1-0.5s (LFM2/Granite are recurrent/hybrid → KV is MBs-scale, small files).
- Generation floor (~25-40 tok/s) is unchanged by any caching scheme — that dominates single-doc sessions; the win is the 2-40s → 0.3s per section switch.

## Constraints / Open Items

- Cache files must be keyed by model fingerprint (restore requires identical model/ctx/backend config) — reuse `tool_capability.rs` fingerprint.
- Restored KV must still fit n_ctx alongside tools + history; `gguf.rs` `kv_bytes_per_token` gives the exact break-even per model.
- Section files contain only **clean section prefixes**. Restore overwrites the one live slot; universal history therefore lives in app state and is appended as one small relevant exchange, never baked into every section file.
- Open: where `active_section` comes from (PDF page bounds vs text layout vs headings), when to pre-eval (ingest vs first read), LRU eviction policy, `--slot-save-path` dir placement.

## Work State

### Completed

- Prior task (LFM2 gated call_only / FRIENDLY_RESULTS) shipped and verified — see LFM2_TOOL_CALLING_FIX.md.
- **Section-scoped KV caching v4 implemented (uncommitted):**
  - `prepare_section_slot` and `cache_note_sections` use clean, conversation-free section files only.
  - Eager whole-doc scan on viewer `onSectionsReady` with `ai://section_cache_progress` events and a document-pane progress overlay.
  - A dedicated slot mutex serializes scans and chat; manifests verify `n_saved` and `n_restored` token counts.
  - v4 exact-boundary priming fixes the live `restore succeeded -> cached=0` failure on recurrent LFM2. Isolated BeeLlama/LFM2 verification restored 489 tokens and evaluated only an 11-token appended question.
  - Chat section preparation now uses the same tool-free direct profile as ordinary active-page asks; Operation/Auto use a stable full tool schema matching their real turns. This closes valid-but-unusable profile mismatches.
  - Section prompt text is canonicalized before rendering and filename hashing, so viewer callback whitespace differences cannot force a cold re-prime. Slot manifests are committed atomically, and background scans yield to active chat turns.
  - PDF/EPUB/HTML viewers emit full section lists (active page first for PDF).

### Active

- (none — implementation in place; verification pending)

### Blocked

- (none)

## Next Move

1. Live verification after restart: prepare all sections, switch pages, and ask independent questions. `evaluated_tokens` should be only the short question/template tail. Referential follow-ups may also evaluate the bounded universal-memory tail.
2. Optimize explicit whole-document retrieval separately if needed; it is intentionally outside the section KV fast path.

## Relevant Files

- /home/paper/myelin/.opencode/anchored_summary.md — this plan.
- /home/paper/myelin/src-tauri/src/llama_server.rs — launch args (`--slot-save-path`), slot pinning.
- /home/paper/myelin/src-tauri/src/state.rs — `ask_ai_stream`, `assemble_note_context`, retrieval-backed path (unchanged).
- /home/paper/myelin/src-tauri/src/note_prompt.rs — per-type sectioning (`outline()`).
- /home/paper/myelin/src-tauri/src/gguf.rs — `kv_bytes_per_token` (cache-size math).
- /home/paper/myelin/src-tauri/src/tool_capability.rs — model fingerprint (cache file keys).
- /home/paper/myelin/src-tauri/openharn-myelin — sidecar loop (consumes the section context).
