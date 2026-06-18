# Myelin note assistant — system prompt & tool-calling reference

This documents exactly what the local model is told, what tools it can call, and
the rules that decide **what ends up in the note vs. what ends up in the chat**.
It is generated from the source of truth:

- System preamble: [`MYELIN_PREAMBLE`](../src-tauri/src/agent.rs) (`agent.rs`)
- Tools + guard rails: `agent.rs` (`WriteNoteTool`, `AppendNoteTool`, `ReplaceTextTool`, …)
- Prompt assembly: `ask_ai_stream` in [`state.rs`](../src-tauri/src/state.rs)

---

## 1. What the model actually receives each turn

Every chat turn sends the model **three** things, in this order:

1. **System preamble** (`MYELIN_PREAMBLE`) — fixed, cached at startup.
2. **Tool schemas** — the 6 tools below (OpenAI function-calling format).
3. **User prompt** — assembled per turn from the open note + history + question.

### 1a. The system preamble (verbatim)

> *(The literal string begins with `/no_think` to disable Qwen3-style reasoning;
> the rest is below, formatted for readability.)*

```text
You are Myelin's built-in note assistant. You are also a capable general
assistant with broad knowledge of history, art, science, culture, and everyday
topics.

CORE BEHAVIOR (most important):
- Be decisive and DO THE TASK. NEVER ask the user clarifying or permission
  questions about formatting, length, structure, or what to include. Make
  reasonable choices and act immediately.
- Treat replies like "yes", "sure", "ok", "anything", "anything you like",
  "you decide", "go ahead" as approval to proceed RIGHT NOW with your best
  version.
- You have extensive general knowledge. Answer factual or general questions
  (e.g. "describe the Mona Lisa") directly and fully from your own knowledge.
  NEVER say you cannot browse the internet, cannot access your training data,
  or need to search — just give the answer.
- Put the COMPLETE, full-length content (the entire essay/poem/list itself)
  into the tool's content field — that is the deliverable, NOT your chat reply.
- After a tool succeeds, STOP. Reply with ONLY a brief one-line confirmation
  (e.g. "Done — I've written it to the note."). Do NOT repeat, rewrite, or
  re-compose the content in your chat reply, and do NOT call more tools or
  re-read/verify the note.
- If a tool reports an error or a refusal, tell the user exactly what went
  wrong. NEVER claim success when a tool did not succeed.
- Do not repeat the same question or the same tool call. Make progress on
  every turn.

WRITING NOTES:
- When the user asks you to write, create, draft, add, generate, rewrite, edit,
  format, reformat, restructure, clean up, fix, improve, or change the note —
  including short requests like 'format this', 'clean this up', 'make it nicer',
  'fix the formatting' — IMMEDIATELY call write_note (or append_note to extend
  existing content) with the COMPLETE, finished text. These requests always
  refer to the OPEN note; never reply that you lack a tool for this. Do not ask
  what to include — just produce the full updated note in Markdown.
- write_note, append_note and replace_text ALWAYS act on the note currently
  open in the editor. You do NOT need to read or search for it first. Pass the
  title shown in 'Open Note:' and the full content; one call is enough.
- The content field must be the actual final text — never a description of what
  you did, and never a placeholder.
- Use replace_text to change a specific snippet; use write_note with an empty
  string to clear the note.

TOOLS (only when actually needed):
- search_notes: ONLY to find OTHER notes by keyword when the user explicitly
  refers to them. Never to interpret a message or read the currently open note
  (its contents are already provided below).
- fetch_web_page: only when the user gives a URL.
- Greetings and small talk ("hi", "gg", "thanks"): reply briefly in chat with
  no tools.
```

### 1b. The per-turn user prompt (assembled in `ask_ai_stream`)

The note body and chat history are injected as **reference-only context**, then
the actual question is appended. The shape is:

```text
The note currently open is titled "<title>".
Its current content (reference only — do NOT copy this as your answer):
<note body, truncated to NOTE_BODY_PROMPT_LIMIT>

Earlier in this conversation:
<chat history>

User request: <the user's latest message>
```

Key details:
- If the note is empty, the body line is replaced with `It is currently empty.`
  (so the model doesn't echo `(empty)` as its answer).
- The body is labeled **"reference only — do NOT copy this as your answer"** so
  the model doesn't just regurgitate the note into the chat.
- `temperature` is kept **low** (configurable) so a small model stays decisive
  and doesn't loop on clarifying questions.
- `max_turns` bounds the tool-call/response loop.

---

## 2. The tools

Registered in `build_myelin_agent`, in this order. All note-mutating goes
through the **single `write_note`** tool, which **always acts on the currently
open note** — the model never has to search for it.

| Tool | Purpose | Writes to note? |
|------|---------|-----------------|
| `read_note` | Read a note's full markdown by ID | no |
| `write_note` | Write / append / edit the open note (one tool, `mode` param) | ✅ |
| `fetch_web_page` | Fetch readable text from a URL | no |
| `search_notes` | Keyword-search **other** notes in the workspace | no |

### The one note tool: `write_note(content, mode?, find?)`

Previously this was three tools (`write_note` / `append_note` / `replace_text`);
they were **merged into one** because a small local model handles fewer tools
far more reliably. The `mode` argument selects the operation (the model decides
which based on the description — there is no hard-coded router):

- **`mode: "replace"`** (default) — set the **entire** note body to `content`.
  Used for write / rewrite / format / reformat. An empty `content` **clears**
  the note.
- **`mode: "append"`** — add `content` to the **end** of the note (the model
  sends only the new text, not the whole body).
- **`mode: "edit"`** — replace the exact `find` snippet with `content`
  (empty `content` deletes the match). For small targeted changes. Requires an
  exact `find` match; if `find` is empty or not present in the note, the tool
  returns a corrective message and saves nothing.

`content` is the single payload field across all modes, so the model only has to
learn one parameter shape. The `find` field matters only for `edit`.

---

## 3. The note-vs-chat decision (the important part)

The split is enforced both by the **prompt instructions** and by **server-side
guard rails**, so a misbehaving model can't dump the deliverable into the chat
or write junk into the note.

### Rule 1 — the deliverable goes in the tool, the chat gets a one-liner
The preamble is explicit: put the *complete content* into the tool's `content`
field; after the tool succeeds, reply with **only a brief one-line
confirmation** and stop. So:

- **Note** ← the full essay / poem / list / reformatted text (via the tool).
- **Chat** ← "Done — I've written it to the note." (a confirmation, nothing more).

### Rule 2 — small talk and questions never touch the note
Greetings, thanks, and factual/general questions are answered **in chat with no
tool call**. The note is only modified when the user actually asks to change it.

### Rule 3 — server-side rejection of "wrong content" (in `agent.rs`)
Before saving, `write_note` rejects content that is clearly meant for the chat,
not the note (skipped for an `edit`-delete, where empty `content` is intended).
Two filters guard this:

- **`looks_like_placeholder`** — rejects bodies containing `[insert…`,
  `placeholder`, `write the poem here`, `add the poem here`. The tool returns a
  refusal telling the model to call again with the real content.
- **`looks_like_meta_status`** — rejects single-line *status sentences* that
  describe the action instead of being the content, e.g. "I have written the
  poem to your note", "Here is the essay", "The note has been updated…". These
  belong in the chat, so the tool refuses and asks for the actual body.

If either fires, **nothing is saved**; the model gets the refusal text back and
must retry with the proper content.

### Rule 4 — the note write is a precise, scoped operation
- `write_note` resolves the target via `resolve_chat_target_note`, which
  **always prefers the currently open note** (by `current_note_id`), falling
  back to exact-title match.
- Creating brand-new notes from the sidebar chat is **not allowed** — the
  assistant can only write to / append to / edit the open note.
- In `edit` mode it verifies the `find` text actually exists before editing;
  otherwise it returns a "couldn't find that text" message (no save).

### Rule 5 — optional human approval gate
If "require tool approval" is on (`is_tool_approval_required`), every mutating
tool call emits `ai://tool_approval_request` and **blocks** until the user
approves/rejects in the UI. A rejection returns "User rejected this action."
and nothing is written.

> Note: `latest_chat_allows_note_mutation()` currently returns `true` — an
> earlier heuristic gate that has been retired in favor of letting the model
> decide (the guard rails above still apply).

---

## 4. How the streams reach the UI (real token streaming)

The chat path does **not** go through `rig`'s agent. `rig` accumulates a tool
call's `arguments` internally and only yields the *fully assembled* arguments
once complete — so the whole note would arrive at once and the UI had to fake a
typewriter. Instead, [`stream_chat::run_chat`](../src-tauri/src/stream_chat.rs)
talks to `llama-server`'s `/v1/chat/completions` directly with `stream: true`,
reads the SSE deltas itself, and runs the multi-turn tool loop by hand (reusing
the rig `Tool` impls only to *execute* tools). This lets it surface the
`write_note` `content` field **as it is generated**.

The backend emits distinct events so the frontend routes them correctly:

| Event | Meaning | UI destination |
|-------|---------|----------------|
| `ai://chat_chunk` | streamed assistant text token(s) | **chat panel** (appended live) |
| `ai://chat_tool` | "a tool is running" with a preview | chat panel (tool chip) |
| `ai://tool_approval_request` | approval needed before a write | approval card |
| `ai://note_stream_start` | a whole-body note write is beginning | **note editor** cleared, preview begins |
| `ai://note_delta` | the next token(s) of the note body | **note editor** (appended live) |
| `ai://note_stream_cancel` | the write turned out to be append/edit, not replace | editor restored to pre-stream body |
| `ai://note_written` | the write_note tool saved (`mode: write\|append`) | **note editor** set to the authoritative final body |

Streaming policy:
- **`replace`** (whole-body rewrite/format — the common case) streams
  token-by-token into the editor via `note_stream_start` → `note_delta`…, then
  `note_written` sets the authoritative final body.
- **`append` / `edit`** do **not** live-stream (a partial preview would be
  misleading); if a stream optimistically started as replace and the mode turns
  out otherwise, `note_stream_cancel` restores the editor. The final
  `note_written` applies the real change instantly (no fake animation).
- The backend extracts the growing `content` value from the **partial** tool-call
  arguments JSON (`extract_partial_content`), stopping before any incomplete
  escape so a half-decoded character is never emitted.

Chat assistant text and note content remain physically separate channels — the
full deliverable lands in the note, only the one-line confirmation lands in chat.

---

## 5. Startup cache warm-up (why the prefix is duplicated)

On startup, `spawn_cache_warmup` sends one request whose **system message =
`MYELIN_PREAMBLE`** and whose tool list = `tool_specs()` (a hand-mirrored copy of
the live `Tool::definition`s). This pre-fills the server's prompt cache so the
first real chat turn reuses the cached system+tools prefix instead of recomputing
it. That is the *only* reason `tool_specs()` exists separately — it must be kept
byte-for-byte in sync with each tool's `definition()`.
