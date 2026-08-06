# Myelin model benchmark — 2026-08-06

This benchmark compares the two local models currently available for Myelin's
Chat and tool path:

- `LFM2-8B-A1B-UD-Q2_K_XL.gguf` through BeeLlama CPU;
- `LFM2.5-2.6B-Q4_K_M.gguf` through BeeLlama CPU.

Both servers used four threads, a 4096-token benchmark context, one slot,
`--reasoning off`, and the model-specific Myelin templates. The benchmark used
the actual `openharn-myelin` streaming protocol and exercised direct Chat,
repeated Chat, `write_note`, `search_notes`, and `read_note` against a temporary
workspace. No user notes were modified.

Run it again with:

```bash
PYTHONUNBUFFERED=1 python3 scripts/benchmark-myelin-models.py \
  --model /home/paper/Downloads/LFM2-8B-A1B-UD-Q2_K_XL.gguf \
  --model /home/paper/Downloads/LFM2.5-2.6B-Q4_K_M.gguf \
  --template src-tauri/templates/lfm2.jinja \
  --template src-tauri/templates/lfm25.jinja \
  --tool-strategy prompt \
  --llama /home/paper/.local/share/com.paper.myelin/bin/bee/cpu/llama-server \
  --sidecar src-tauri/resources/bin/openharn-myelin
```

## Results

### LFM2 with prompt tools + strict grammar

| Case | First delta | Total | Prompt | Cached | Evaluated | Result |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| Chat cold | 5.86 s | 7.53 s | 394 | 0 | 394 | prose answer |
| Chat follow-up | 5.82 s | 6.20 s | 395 | 0 | 395 | prose answer |
| `write_note` | 2.82 s | 13.82 s | 408 | 255 | 153 | pass |
| `search_notes` | 0.97 s | 8.19 s | 449 | 391 | 58 | pass |
| `read_note` | 1.09 s | 8.86 s | 451 | 393 | 58 | pass |

The tool calls were reliable with the prompt-tools profile. After a tool result,
the next turn reused about 87% of the prompt. The cold Chat case is not a
prepared section-cache measurement.

### LFM2.5 with prompt tools + strict grammar

| Case | First delta | Total | Result |
| --- | ---: | ---: | --- |
| Chat cold | 7.49 s | 9.31 s | prose answer |
| Chat follow-up | 8.03 s | 14.08 s | prose answer |
| `write_note` | 5.92 s | 7.69 s | failed to call tool |
| `search_notes` | 7.83 s | 9.56 s | failed to call tool |
| `read_note` | 10.01 s | 11.62 s | failed to call tool |

The model-specific `lfm25.jinja` template was applied. The failure was not a
missing template: this model did not follow the prompt-tool protocol under the
same settings.

### LFM2.5 with native tools

| Case | First delta | Total | Result |
| --- | ---: | ---: | --- |
| Chat cold | 8.28 s | 10.40 s | prose answer |
| Chat follow-up | 8.73 s | 14.32 s | prose answer |
| `write_note` | 4.03 s | 34.11 s | pass |
| `search_notes` | 3.92 s | 23.57 s | pass; made an extra read call |
| `read_note` | 1.08 s | 20.71 s | pass |

Native tools recovered correctness, but tool turns were much slower and less
focused. The extra `read_note` call on the search request is a model behavior
issue, not a Myelin authorization failure.

## Decision

Use these settings as the starting profiles:

- LFM2: `toolMode: "prompt"`, strict prompt tools enabled;
- LFM2.5: `toolMode: "native"`, strict/prompt tools disabled.

For Myelin's current priority—fast, predictable local interaction—LFM2 is the
better production tool model in this environment. LFM2.5 is smaller but had
slower Chat first-token latency and substantially slower native tool turns.
It should remain experimental until its native-call latency and unnecessary
extra-call behavior are improved.

## Caveat about cache numbers

The benchmark's repeated Chat case starts from a cold dirty slot. It is useful
for detecting server behavior, but it is not equivalent to Myelin's prepared
section cache, whose clean prefix is restored before the request. Do not use
the 0% cached value from those two rows to conclude that production section
preparation is broken. A separate prepared-prefix fixture should be used for
cache comparisons.

## SmolLM3 3B Q4_K_M runtime comparison

Downloaded from `smarttasks/SmolLM3-3B-GGUF` and verified against the published
SHA-256:

```text
bf342c11ef16312997beaa48068c4e1b5a319ce83873c9602e1a6a4a01eaa8d2
```

The same benchmark was run with the model's built-in chat template, prompt
tools, four CPU threads, one slot, 4096 context, and reasoning disabled.

| Runtime | Chat first | Chat follow-up | Write first / total | Search first / total | Read first / total | Tool calls |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| Stock llama.cpp | 1.87 s | 0.51 s | 4.42 / 27.54 s | 1.59 / 9.71 s | 1.63 / 4.91 s | all expected calls passed |
| BeeLlama | 1.51 s | 0.54 s | 4.41 / 29.75 s | 1.48 / 9.77 s | 1.33 / 5.69 s | all expected calls passed |
| Maple | 2.04 s | 0.61 s | 4.99 / 31.14 s | 1.55 / 10.75 s | 1.75 / 5.97 s | all expected calls passed |

Cache accounting was consistent across the runtimes: the Chat follow-up reused
371 of 386 prompt tokens (96.1%). Tool requests reused approximately 89% of
the stable prompt for search/read and 61–63% for Write. The first open-ended
Chat request incorrectly selected `web_search` once because all tools were
offered; this is a model/tool-intent behavior, not a runtime failure.

Conclusion: use stock llama.cpp as the SmolLM3 profile runtime. Bee was close
but not faster overall, and Maple was slower for every measured case. No GPU
device was exposed on this machine, so this comparison is CPU-only.

## Maple TQ2_0 investigation

`/home/paper/Downloads/maple-tq2_0.gguf` was tested with the Maple runtime.
The model is a Maple architecture model and its embedded template starts an
assistant `<think>` block. `--reasoning off` alone does not stop generation;
the model needs `/no_think` in the system prompt. A direct no-tool request then
measured approximately 145 prompt tokens/s and 58 generation tokens/s.

With Myelin's full tool-bearing prompt, the model requires native tool mode.
Prompt-tool mode produced no parseable content. Native mode produced tool calls,
but had these problems:

- Chat still emitted reasoning-only output when the benchmark supplied tools.
- `write_note` produced malformed JSON containing an unterminated long note.
- `search_notes` made extra calls (`search_notes`, `read_note`, `search_notes`).
- `read_note` called correctly.

The native benchmark completed with first-token times of approximately 4.5 s for
Write, 2.0 s for Search, and 1.6 s for Read, but it is not currently a reliable
Myelin profile because of the malformed Write call and chat reasoning behavior.

### TQ2 production-config retest

The first TQ2 benchmark was too pessimistic because its fixture did not mirror
Myelin's `friendly_results` routing or focused `targeted_write` flag, and it used
an artificial 256-token cap. The corrected fixture uses the actual native path,
`friendly_results`, `noThink: true`, focused Write targeting, a 1,024-token cap,
and the exact one-tool Write contract.

The working profile is:

```json
{
  "toolMode": "native",
  "strict": false,
  "promptTools": false,
  "callOnly": true,
  "noThink": true,
  "templateKwargs": "{\"enable_thinking\":false}"
}
```

The final exact-config run (native mode, `noThink: true`, template kwargs,
without an extra `/no_think` prompt marker) produced:

| Case | First delta | Total | Cached | Evaluated | Result |
| --- | ---: | ---: | ---: | ---: | --- |
| Chat cold | 5.61 s | 6.18 s | 0 | 635 | prose answer |
| Chat follow-up | 8.98 s | 9.54 s | 623 | 13 | prose answer; 97.96% reuse |
| Focused Write | 4.32 s | 11.91 s | 0 | 447 | one valid `write_note` call |
| Search | 4.14 s | 15.67 s | 692 | 203 | expected `search_notes` plus one extra `read_note` |
| Read | 1.96 s | 5.72 s | 597 | 105 | expected `read_note` |

The focused Write call is now valid; the earlier malformed JSON was caused by
the benchmark's 256-token cap truncating the generated argument. The production
tool budget is larger. The first Write row is intentionally cold because this
fixture does not prime a saved Write section prefix before the request; the real
section preparation path does that separately.

Prompt-tool mode was also tested. It preserved Chat and focused Write, but TQ2
failed to produce reliable Search and Read calls, so it should not be used for
this model. Native mode is the shipping candidate.

The final TQ2 run with the benchmark aligned to the host's one-call deterministic
tool policy was consistent: Chat cold 5.80 s first delta / 6.36 s total, Chat
follow-up 8.86 s / 9.48 s with 97.96% prompt reuse, focused Write 4.28 s / 12.28
s with one valid `write_note`, Search 3.64 s / 14.32 s, and Read 1.90 s / 5.33
s. Search can still make a second turn because the harness's per-turn limit is
separate from the overall agent loop; the production targeted Write path remains
one mutation call.

The targeted-write format override was then corrected. Focused Write now honors
the configured tool profile instead of always selecting native calling. With
`configs/ai-config.lfm.json` (`toolMode: "prompt"`, `strict: true`), LFM2 emitted
one valid `write_note` call with 255 cached and 153 evaluated tokens (62.5% reuse);
Search and Read also emitted exactly their expected calls. Maple keeps its native
profile independently, so the two models can ship with different tool formats.

## Maple Preview TQ1_0-head Q4_K test

Downloaded from the user-provided Hugging Face repository:

```text
https://huggingface.co/deepgrove/maple-preview-GGUF
```

The file is 4,984,016,416 bytes and its local SHA-256 is:

```text
54016e4d543bd688829e67103fc85b8396db94b7f8eb3f81fa95884e44393872
```

The existing Maple binary at
`/home/paper/cpp/maple-llama/build/bin/llama-server` loads this TQ1 file without
a rebuild. It also passes the slot persistence check: a 91-token slot saved to
`tq1-test.bin` and restored with exactly 91 tokens. Therefore a different llama
binary is not required for basic model loading or save/restore.

The server reports that `cache_reuse` is not supported by this model context and
disables that flag. The newer server-side context checkpoints still operate, but
the explicit cache-reuse chunk setting is not active for this model.

The model is not currently suitable as a Myelin production profile:

- With `/no_think`, direct text works at approximately 94 prompt tokens/s and
  49 generation tokens/s in this CPU configuration, but the model still emits a
  separate `reasoning_content` stream.
- With Myelin's native tool path, it did not produce a `write_note` call. A raw
  one-tool request instead generated prose repeatedly until the 512-token limit.
- Through the actual Myelin sidecar benchmark, Chat produced no usable content
  before the 256-token limit, Write produced prose instead of the tool call,
  Search called the expected tool but then made an extra `read_note` call, and
  Read called correctly.
- The native sidecar results were approximately: Chat cold 10.6 s, Chat
  follow-up 7.1 s, Write 7.0 s, Search 19.2 s, and Read 5.7 s. Those Chat rows
  are not useful latency results because generation was spent in reasoning.

The config is preserved for further experiments at
`configs/ai-config.maple-preview-tq1-q4.json`, using native tools and the
`/no_think` setting. The official Maple Preview documentation also warns that
the preview is focused primarily on raw reasoning and may underperform on
agentic benchmarks; this matches the tool behavior observed here:
https://huggingface.co/deepgrove/maple-preview/raw/main/README.md
