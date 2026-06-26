# Myelin note assistant — system prompt & tool-calling reference

How the local model is driven for the in-app chat. The app is **model-agnostic**
— it runs any capable local GGUF model — and the implementation is deliberately
small: a minimal preamble, **per-message tool gating** (toggleable), and a plain
streaming tool loop. Earlier, heavier machinery (harvest backstop, forced-write,
meta/clear overrides) was deleted once it proved unnecessary with a model that
calls tools reliably.

Source of truth:
- Preamble + tools + gating: [`agent.rs`](../src-tauri/src/agent.rs)
- Streaming chat loop: [`stream_chat.rs`](../src-tauri/src/stream_chat.rs)
- Prompt assembly: `ask_ai_stream` in [`state.rs`](../src-tauri/src/state.rs)

---

## 1. What the model receives each turn

1. **System preamble** (`MYELIN_PREAMBLE`) — fixed, minimal.
2. **Gated tool list** — only the tools the message warrants (see §3).
3. **User prompt** — the open note's content + recent history + the request.

### 1a. The preamble (verbatim)

```text
You are the assistant inside Myelin, a local notes app. The text of the note
currently open in the editor is included in the user's message — you already
have it.

- To change the open note (write, rewrite, edit, format, add to, shorten,
  clear, etc.), call write_note with the full result. Don't just describe the
  change in chat — make it with the tool.
- Use fetch_web_page only when the user gives a URL, and search_notes only when
  the user asks about your other notes. For greetings or general questions, just
  reply briefly — do not read, search, or fetch.
```

### 1b. The user prompt (assembled in `ask_ai_stream`)

```text
The note currently open is titled "<title>".
Its current content (reference only): <body, truncated to a budget>

Earlier in this conversation: <recent history>

User request: <the user's message>
```

The note body is given as context so the model can edit it; an empty note says
so explicitly.

---

## 2. The tools

Four tools (defined in `agent.rs`; the same OpenAI specs are mirrored in
`tool_specs()` for the startup warm-up and filtered by gating):

| Tool | Purpose |
|------|---------|
| `write_note(content, mode?, find?)` | Edit the **open** note only — never creates a new note |
| `read_note(note_id)` | Read **another** note by id (ids from `search_notes`) |
| `search_notes(query)` | Find **other** notes in the workspace |
| `fetch_web_page(url)` | Fetch a public web page's text |

`write_note` modes (decided by `plan_write`, which is pure + unit-tested):
- **`replace`** (default) — set the whole body to `content` (empty clears it).
- **`append`** — add `content` to the end.
- **`edit`** — replace the exact `find` snippet with `content` (empty deletes it).

`plan_write` is tolerant of model slips: it infers intent from the fields
(an explicit `replace` ignores a stray `find`; a `find` with no mode is a
snippet edit), does a whitespace-tolerant `find` match, and **strips prompt
markers** the model may echo into content (e.g. a model echoing the prompt's
`--- CURRENT NOTE ---` framing).

---

## 3. Per-message tool gating (the key idea)

Instead of sending every tool every turn and steering with the prompt, we hand
the model **only the tools its message warrants** — the model can't misfire
on a tool it was never given. (Pattern proven in the `ggufplay` experiment.)

`agent::select_tools(message, has_open_note, edit_thread)`:
- **small talk** (`is_small_talk`, ≤4 ack words) → **no tools** (just chat).
- **write intent** (`note_write_intent` — edit verbs / transform phrasings /
  affirmations / note-targeted soft verbs) → **`write_note`**.
- **other-notes intent** (`wants_other_notes`) → **`search_notes` + `read_note`**.
- **fetch intent** (`wants_fetch` — a URL, a **bare domain** via a TLD allowlist
  like `example.com` while excluding file names like `notes.md`, or "fetch/open
  the page") → **`fetch_web_page`**.
- a pure question with no action intent → **no tools** → the model answers in chat.

### Two independent toggles (Settings → Assistant Tooling)

Gating and the deterministic correctness tools are **separate** switches, so each
can suit the model in use (`select_tools_cfg(message, has_open_note, edit_thread,
gating, deterministic)`):

- **Per-message tool gating** (`gating`) — the selection above. Off → the model
  gets the full general toolset every turn and decides for itself (suited to
  larger, more capable models).
- **Deterministic format & find** (`deterministic`) — routes a clean structural
  cleanup to the regex **`format_note`** tool (instead of an LLM rewrite) and a
  word lookup to **`find_in_note`**, and enables the destructive-write guard.
  These are *correctness* assists, not a gating crutch, so the format override
  applies **whether or not gating is on** — formatting stays reliable even with
  the full toolset offered.

(Configs from before the split have no `tool_gating` field; it falls back to the
old combined `deterministic_tools` value so existing behavior is preserved.)

**Edit-thread context (the "New note 18" fix).** Per-message gating looks only at
the latest message, so a verb-less follow-up correction ("no thats wrong", a
typo'd "formate it") would lose `write_note` and the model could only chat a fake
"done". `in_edit_thread(recent_user_messages)` is true when any of the last few
user turns carried write intent; `ask_ai_stream` derives it from the note's chat
history and passes `edit_thread`. When set, `write_note` stays available even
without a fresh verb, so corrections keep editing the note. (Some small models'
unreliable `#`-heading syntax on terse corrections is a separate **model ceiling**,
not a gating gap — a preamble nudge didn't move it, so the preamble stays minimal.)

`run_chat` omits the `tools`/`tool_choice` fields entirely when the gated list is
empty, so the model simply replies.

---

## 4. The streaming loop (`run_chat`)

A plain multi-turn loop, talking to `llama-server`'s `/v1/chat/completions`
directly (not through rig) so note content can stream token-by-token:

1. POST with `stream: true` and the gated tools; read the SSE deltas.
2. Stream assistant text live via `ai://chat_chunk`.
3. Accumulate tool-call deltas by `index` (handles multiple calls per turn).
4. For a `write_note` whole-body replace, surface the growing `content` field as
   `ai://note_stream_start` → `ai://note_delta` so the editor fills in live
   (append/edit don't live-stream; `note_written` reconciles).
5. When the turn has tool calls: append the assistant tool-call message, execute
   each tool (reusing the `Tool` impls, which save the note and emit the chip +
   `ai://note_written`), append the results, and loop. No tool calls → done.

UI events: `chat_chunk` (assistant text), `chat_tool` (tool chip),
`note_stream_start`/`note_delta`/`note_stream_cancel` (live note),
`note_written` (authoritative save), `chat_done`/`chat_error`.

There is **no** harvest/backstop/forced-write/meta-strip/clear-override anymore.
The note changes only when the model calls `write_note` and it succeeds.

---

## 5. Testing it headlessly (no GUI)

- **Logic** — `cargo test -p myelin --lib` covers `select_tools`, `is_small_talk`,
  `plan_write`, `find_tolerant`, `strip_prompt_markers`, and the partial-JSON
  streaming extractors.
- **End-to-end** — [`src-tauri/src/bin/tool_e2e.rs`](../src-tauri/src/bin/tool_e2e.rs)
  drives a real `llama-server` through the same gated loop against a scratch
  note store, per tool + content hammers:
  ```
  cargo run --bin tool_e2e -- <model.gguf> [llama-server-bin] [port]
  ```

`AppState` is Wry-bound so the harness mirrors note storage on a scratch store;
writes go through the real `plan_write` and gating through the real
`select_tools`.
