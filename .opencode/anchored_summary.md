## Objective

- Make Myelin's LFM2 model actually write notes (the "new note 13" empty-note bug) while keeping greetings as natural prose. User pointed me to LFM2_TOOL_CALLING_FIX.md and, after I shipped a blunt global `call_only`, asked to gate it on intent (port openharn FRIENDLY_RESULTS) — "option 1".

## Important Details

- openharn at /tmp/opencode/openharn, branch `myelin-tools`. Sidecar = /home/paper/myelin/src-tauri/openharn-myelin (bin `openharn-myelin`). llama-server: /home/paper/.local/share/com.paper.myelin/bin/cpu/llama-server (v9585). LFM2 template: /home/paper/myelin/src-tauri/templates/lfm2.jinja.
- **Root causes found & fixed for LFM2-8B-A1B-UD-Q2_K_XL (2-bit quant):**
  1. GBNF `string` rule `[^"\\]` allowed literal newlines → model emitted poem with raw `\n` inside JSON string → invalid JSON → `parse_text_tool_calls` dropped the call. **Fix:** `string ::= "\"" ( [^"\\\n\r\t] | ... )* "\""`.
  2. Weak model evades the `text` branch (picks prose even at temp 0). **Fix:** `call_only` grammar (no text branch), selected via `Options.call_only`.
  3. Model loops on whitespace (`ws ::= [ \t\n]*`) — emits `<tool_call>` then infinite spaces, never reaches `[`. **Fix:** `ws ::= [ \t]?`.
  4. Global `call_only` is a blunt hammer (greetings also forced to call tools — the over-eager-tool bug FRIENDLY_RESULTS prevents). **Fix (option 1):** gate `call_only` on a CHAT/TOOL intent classification (ported `run_intent_detection` from openharn). CHAT → prose; TOOL → call_only forces the call. Default to TOOL on ambiguity (losing a note-write > a spurious greeting call).
- `OPENHARN_FRIENDLY_RESULTS` (openharn docs/adapting-openharn.md:41): classifies each turn CHAT/TOOL before the tool loop; CHAT skips tools. Requires prompt_tools. Myelin's fork did NOT have it; now ported.
- LFM2.5-8B-A1B-APEX-I-Compact: works with plain `strict`+`prompt_tools` (emits closed `<tool_call>`), no forcing needed.

## Work State

### Completed

- Read LFM2_TOOL_CALLING_FIX.md; confirmed its prompt-tools+strict forcing is implemented.
- Fixed invalid-JSON root cause (string rule excludes `\n\r\t`).
- Tightened `call` rule (no trailing `ws` before `]`/`,`); changed `ws ::= [ \t]?` to stop whitespace loops.
- Added `Options.call_only` + `Options.friendly_results`; async `run_intent_detection` (one tiny non-streaming call, defaults TOOL on ambiguity); CHAT turns skip the tool loop (no grammar, no tool recovery); grammar uses `tool_grammar_call_only` only when `force_tool.is_some() || (call_only && intent_is_tool)`.
- sidecar.rs sets `call_only` + `friendly_results` = `force_prompt_tools` for `prefersPromptTools` (LFM2) profiles.
- **Live-verified on LFM2-8B Q2_K_XL (temp 0):** greeting → prose (GOOD); "write a poem about the moon" / "save to my note" / "write a story" / "what is in note 13" → `write_note` call (GOOD); only "how do I use this app" over-triggers (intentional default-TOOL tradeoff). 5/6 ideal, no lost note-writes.
- Unit (6) + e2e (1) tests pass. Release binary rebuilt + reinstalled to resources/bin/openharn-myelin. Myelin `cargo check` clean. Updated LFM2_TOOL_CALLING_FIX.md addendum. Test servers stopped.

### Active

- (none) — option 1 implemented and verified.

### Blocked

- (none)

## Next Move

1. (done) Intent-gated `call_only` (FRIENDLY_RESULTS) for LFM2: greetings → prose, note-writes → forced `write_note`. Optionally verify in Myelin UI with the real model.
2. (optional) Tighten intent prompt / add keyword fallback if "how do I use this app"-style questions should stay CHAT.
3. (optional) `npm run check` / tauri build to ship. No code changes pending in the sidecar.

## Relevant Files

- /home/paper/myelin/LFM2_TOOL_CALLING_FIX.md — root-cause doc (updated addendum: string fix, ws fix, call_only, friendly_results).
- /home/paper/myelin/src-tauri/openharn-myelin/src/harness.rs — GRAMMAR_TAIL: `string` excludes `\n\r\t`; `ws ::= [ \t]?`; `call` rule trailing-ws removed.
- /home/paper/myelin/src-tauri/openharn-myelin/src/agent.rs — `Options.call_only`+`friendly_results`; `run_intent_detection`; CHAT-skip + gated grammar.
- /home/paper/myelin/src-tauri/src/sidecar.rs — sets `call_only`+`friendly_results`: `force_prompt_tools`.
- /home/paper/myelin/src-tauri/model-profiles.json — `prefersPromptTools: true` for LFM2.
- /home/paper/Downloads/LFM2-8B-A1B-UD-Q2_K_XL.gguf — now works (gated call_only + fixes).
- /home/paper/Downloads/LFM2.5-8B-A1B-APEX-I-Compact.gguf — works with plain strict+prompt_tools.
- /home/paper/myelin/src-tauri/resources/bin/openharn-myelin — rebuilt + reinstalled (release).
