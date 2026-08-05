# Myelin Section KV Cache: Technical Findings

This note records the section-cache architecture, the difficult bugs we found, and the reasoning behind the current design. It is intended as source material for future engineering notes, technical blogs, and product messaging.

## The problem

Small language models running locally can answer quickly once they reach generation, but evaluating a long document into the model context can dominate time-to-first-token. Repeating that prefill for every question or every page makes the assistant feel slow even when the answer itself is short.

The goal is not to keep the entire document and every conversation branch in one giant context. The goal is to evaluate each useful document section once, persist its clean model state, and restore only the section the user is currently asking about.

## Current architecture

Myelin uses llama.cpp slot APIs in two layers:

1. Slot `0` is the one live runtime context. It is the active KV state used by the next request.
2. Each prepared section is serialized from slot `0` into a separate `.slot` snapshot through `/slots/0?action=save`.
3. When the user asks about a section, its snapshot is restored into slot `0` through `/slots/0?action=restore`.
4. A paired JSON manifest records the cache identity and the exact saved token count.

The files are therefore persistent slot snapshots, not independent inference sessions. The application owns one active slot and swaps clean section snapshots into it.

Each snapshot contains only the clean, reusable system/tool/section prefix. It does not contain the user question, assistant answer, tool results, or section-specific conversation history.

## Request lifecycle

### Preparation

Viewer components emit sections: PDF pages, EPUB chapters, HTML buckets, or other active document regions. The backend:

- normalizes the section text;
- creates a profile-specific system prompt and tool schema;
- computes a cache identity from the model, executable, template, context size, backend flags, system prompt, tools, and cache-wire revision;
- asks llama-server to render two otherwise identical prompts whose user content differs at the first character;
- takes the exact common prefix of those rendered prompts;
- evaluates that raw prefix with `/completion` and `n_predict: 0`;
- saves the resulting slot snapshot;
- accepts the cache only when `n_saved > 0` and the manifest commits successfully.

The two sentinels are deliberately different at character zero. A shared sentinel prefix would become part of the saved KV and cause the real user question to diverge from the snapshot.

### Asking a question

For an ordinary Chat question about the visible section:

1. Restore the clean section snapshot into slot `0`.
2. Add the bounded dynamic tail: the question and, when needed, one relevant exchange from universal conversation memory.
3. Generate the answer.
4. Never save that conversation-bearing state as the section file.

Explicit whole-document, other-page, web, note-search, and similar requests leave the section fast path and use retrieval/tools as appropriate.

## Universal conversation memory

Conversation memory is stored separately from section snapshots. This lets the user ask a follow-up about a previous page after navigating away from it.

For fast document Chat, Myelin selects at most one relevant user/assistant pair, bounded to about 1,200 characters. Pronoun-style follow-ups fall back to the latest pair; unrelated questions do not replay unrelated history into the new section.

Operation mode retains the canonical live tool conversation. Isolated Edit mode intentionally does not retain the conversation because it is a targeted write action.

This separation is the central context tradeoff:

```text
clean section snapshot + small universal dynamic tail + current question
```

The section snapshot remains reusable while the conversation remains universal.

## Cache profiles and selection

Chat and Write do not use the exact same snapshot because their model-facing tool schemas differ.

- Chat active-page questions use the tool-free direct profile.
- Explicit Chat tool requests use the read-only tool profile.
- Edit uses the write profile.
- Operation/Auto use a stable full tool schema; host-side authorization still enforces selection and mutation safety.

Selection text is never baked into the section snapshot. It is placed in the dynamic request tail and checked again by the host when a mutation tool executes. This means changing the selected text does not invalidate the clean page cache.

## Bugs and loopholes we found

### 1. The v3 sentinel polluted every cache

The first implementation rendered two user sentinels with a long shared prefix. The longest common prefix therefore included part of the synthetic sentinel. A real question diverged at that point, so llama.cpp could report a successful restore while the live request showed `cached=0`.

Fix: use sentinels that differ at the first character and save only the exact rendered common prefix.

### 2. A dirty resident slot was mistaken for a clean section

After a model answered, slot `0` contained the previous question and answer. The in-memory marker still claimed that the clean section was resident. A following question on the same page could then reuse an arbitrary partial prefix instead of restoring the clean disk snapshot. This explained inconsistent values such as 40–60% reuse after a cold request.

Fix: every real section request force-restores the clean snapshot. Only speculative page-change warmups may skip a restore when the clean section is definitely still resident.

### 3. Preparation used the wrong Chat profile

The pre-cache initially prepared Chat with read-only tool schemas. Ordinary active-page Chat intentionally removes tools and uses the direct preamble. The snapshot was valid, but its system/tool prefix did not match the real request, so it could not be reused.

Fix: Chat section preparation and active-page warmup use the same tool-free direct profile as the real active-page request.

### 4. Operation/Auto schemas changed with the question

Operation requests selected a question-dependent subset of tools while preparation used a full schema. That made operation caches miss whenever the requested tool set changed.

Fix: Operation/Auto use a stable model-facing schema. The host still applies deterministic intent, selection restrictions, document-type restrictions, and mutation authorization when executing a call.

### 5. Viewer extraction produced equivalent text with different bytes

PDF and other viewers can differ in trailing whitespace between their eager section callback and active-section callback. The prompt then changed by invisible characters, causing a different filename and identity.

Fix: canonicalize section content before rendering and hash exactly the capped text that enters the prompt.

### 6. Context identity used the configured context instead of the running context

Automatic context sizing can launch llama-server with a context size different from the configured value. If preparation fingerprints one size and requests use another, every cache misses.

Fix: identity uses the actual running server context size.

### 7. Slot save/restore could be treated as success without proof

HTTP success alone is not enough. A save can return no tokens, a restore can return fewer tokens than expected, or a manifest can be partially written.

Fixes:

- require positive `n_saved`;
- require exact `n_restored == n_saved`;
- delete invalid snapshots and manifests;
- atomically rename the manifest into place;
- include model/template/backend/context/tool provenance in the identity.

### 8. Background preparation could contend with a real request

The eager scan and live chat both use slot `0`. If the scan takes the slot at the wrong moment, the user request waits behind a page prime.

Fix: serialize all slot operations and make the background scanner yield when a chat turn is already active.

## What the cache does and does not accelerate

The cache removes repeated prompt prefill. It does not eliminate:

- disk restore I/O;
- prompt-template and request construction;
- retrieval for intentionally wider questions;
- model decoding/generation;
- tool execution or external network calls.

The expected win is a large reduction in time-to-first-token after preparation, not zero total response time. Generation speed remains determined by the selected SLM and hardware.

## Verification history

An isolated test using the same BeeLlama/LFM2 stack restored 489 prepared tokens and evaluated only an 11-token appended question. The Rust suite currently passes 143 tests, including cache-boundary, Chat-profile parity, and stable Operation-schema regression tests.

## Retest checklist

After a cache-wire or prompt-profile change:

1. Restart/rebuild the app.
2. Reopen the document.
3. Wait for the new `Prepared X/X` terminal event.
4. Ask a normal question on the active page.
5. Confirm the log contains `restored section slot` before the model request.
6. Confirm `cached` is close to the prepared prefix and `evaluated` is limited to the question/history tail.
7. Ask again on the same page and confirm it restores cleanly rather than relying on accidental dirty-slot reuse.
8. Switch pages and repeat.

