# Performance Optimizations

## Background

Myelin felt slower than running `llama-cli` directly. The root cause was not the
UI layer or HTTP overhead — it was that the app was doing **much more work per
user message** than a single `llama-cli` invocation, and some of that work was
invisible to the user.

## Changes made

### 1. Prompt restructuring for KV cache reuse (biggest win)

**Problem:** The note context (the body of the open note) was included in the
last **user message** on every request. The saved conversation history stored
only the raw question text (without the note). This meant `cache_prompt` in
llama-server could match **only the system prompt** (~500 tokens). The entire
note + conversation history + question was re-evaluated from scratch on every
single turn — a 10K-token note alone cost ~4s of prompt evaluation time.

**Fix:** Moved the note context into the **system message**, which is identical
across requests when the note hasn't changed. The system message is now:
`MYELIN_PREAMBLE + "\n\n" + note_context`. The user message is just the raw
question text. `cache_prompt` now matches the full system message + conversation
history, and only the short question needs re-evaluation each turn.

**Files:**
- `src-tauri/src/state.rs` — prompt assembly restructured
- `src-tauri/src/state.rs` — comment updated, `user_content` removed

### 2. Server warm-up on app startup

**Problem:** The first chat request paid ~30s cold-start cost (model loading,
prompt evaluation). The `warm_llama_server` command was only called when a note
opened, which was often right before the user's first chat.

**Fix:** Spawn a background `warm_llama_server()` call during app startup in
the Tauri `setup` hook. The server loads the model into memory while the UI
loads, so the first chat is near-instant. Best-effort — if it fails, the first
chat starts the server normally.

**Files:**
- `src-tauri/src/lib.rs` — added `tauri::async_runtime::spawn` with warm-up in setup

The model server is started in the background and shared across notes and chat
threads for the lifetime of the app process. This does not share conversation
context: every request sends its own note-scoped system prompt and conversation,
while `cache_prompt` reuses matching token prefixes from the first real request.
The server remains single-slot so requests are serialized and cannot mix
note/tool state. The `ai://llama_warmup` `ready` event means the server is
healthy; no synthetic completion is issued or awaited before the first chat.
The in-memory model and KV cache are released when the app exits and are loaded
again next run.
- Fixed: used `tauri::async_runtime::spawn` instead of `tokio::spawn` (the setup
  callback runs outside a tokio context, causing a panic)

### 3. Eliminated extra intent-classification inference for LFM2

**Problem:** For models with `prefersPromptTools` (e.g. LFM2 at low quants),
every turn ran a **separate model inference** just to classify the user's
request as `TOOL` vs `CHAT` (the `friendly_results` / `run_intent_detection`
path). This doubled the cost of every interaction.

**Fix:** The host (Myelin Tauri backend) now computes the intent using its
existing deterministic heuristics (`note_write_intent`, `in_edit_thread`,
`select_tools_cfg`) and passes `intent_is_tool` to the sidecar. The model-based
fallback is kept for standalone sidecar callers.

**Files:**
- `src-tauri/src/sidecar.rs` — derives `intent_is_tool` from deterministic routing
- `src-tauri/openharn-myelin/src/agent.rs` — uses `opts.intent_is_tool` when present,
  added `intent_is_tool` field to `Options`

### 4. Reused HTTP clients (connection pooling)

**Problem:** A new `reqwest::Client` was created for every request, preventing
HTTP keep-alive from working. The sidecar explicitly disabled pooling with
`pool_max_idle_per_host(0)`, adding a new TCP connection for every model
generation.

**Fix:** Both the main app and the sidecar now use process-wide shared
`reqwest::Client` instances via `OnceLock`. The sidecar uses
`pool_max_idle_per_host(2)` so connections stay alive across the tool loop.

**Files:**
- `src-tauri/src/sidecar.rs` — `http_client()` for health checks and sidecar requests
- `src-tauri/openharn-myelin/src/agent.rs` — `upstream_client()` for llama-server

### 5. Removed per-turn debug file write

**Problem:** The legacy `stream_chat` path wrote a pretty-printed JSON dump of
every request body to a temp file on every model turn.

**Fix:** Removed the `std::fs::write` call.

**Files:**
- `src-tauri/src/stream_chat.rs`

### 6. Added debug window with performance metrics

**What:** A collapsible debug panel above the chat prompt, toggled via a 🐞
button. Shows:

| Metric | Source |
|--------|--------|
| Prompt → First token | Client-side timestamps (first-chunk time − send time) |
| First token → Done | Client-side timestamps (done time − first-chunk time) |
| Total elapsed | Wall clock from send to finish |
| Prompt tokens | From llama-server `include_usage` stream data |
| Completion tokens | From llama-server, or estimated from reply chars (≈¼ tok/char) |
| Tokens/s (gen) | Computed from tokens and generation time |
| Activity trace | Color-coded timeline of events: request sent, generation started, tool calls, note writes, completion |

**Files:**
- `src/routes/notes/[id]/+page.svelte` — state, listeners, template, CSS

### 7. Token usage forwarding

**Problem:** The sidecar requested `include_usage: true` from llama-server but
never parsed the `usage` field from the stream chunks. The frontend had no way
to display token counts.

**Fix:** Added `Out::Usage` variant to the sidecar's event enum. Parsed from the
SSE stream and forwarded through the sidecar SSE server → main app → Tauri event
→ Svelte frontend.

**Files:**
- `src-tauri/openharn-myelin/src/agent.rs` — `Out::Usage` variant, parsed from stream
- `src-tauri/openharn-myelin/src/server.rs` — mapped to `usage` SSE event
- `src-tauri/src/sidecar.rs` — forwarded as `ai://chat_usage`
- `src/routes/notes/[id]/+page.svelte` — listener and display

### 8. Misc

- `.gitignore`: Added sidecar target directory (`src-tauri/openharn-myelin/target`)
- Various auto-formatting changes from `cargo fmt`

### 9. Selective operation context

Operation prompts now omit the existing note body for create, append, prepend,
insert, search, and fetch requests. Full note context remains for rewrites,
formatting, deletion, and other operations that must preserve surrounding text.
An armed editor selection is authoritative: only the selected text is sent to
the model, mutation schemas are limited to `write_note`, and Rust applies the
existing anchor-checked selection splice before saving.

The model-prompt debug event also reports prompt character count and an
approximate token count, making prompt-evaluation regressions visible even when
the request is cancelled before llama-server emits final usage.

### 9. Prompt-latency measurement and CPU profile

The sidecar emits `request_serialized`, `response_headers`, and
`first_model_delta` timing events, plus token usage. This splits the observed
prompt-to-first-delta interval into serialization/HTTP wait and model work.
Tool-call deltas count as a first model delta too. Compacted tool schemas are
alphabetically ordered, and live history is trimmed only at complete turns, so
unchanged notes retain a stable cache prefix; note edits intentionally rebuild
the system message and invalidate that prefix.

Run the CPU sweep with:

```sh
LLAMA_SERVER=/path/to/llama-server MODEL=/path/to/model.gguf ./scripts/benchmark-cpu-profile.sh
```

It compares automatic threading versus six physical-core threads, ubatch
256/512/1024, flash attention on/off, and f16/q8_0 KV. Choose the fastest safe
configuration by prompt-to-first-delta for the same request, not generation
tok/s alone. The runtime ladder still falls back when a build rejects a flag.
