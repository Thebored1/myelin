# LoRA iteration results — Granite-4.0-h-1b, on-device (4 GB RTX 2050, Windows)

All training, data-gen, and eval ran locally: QLoRA on a 4 GB RTX 2050, data
generated free via OpenCode Zen. The hard win was making a Mamba-2 hybrid trainable
on Windows + 4 GB (see [README](README.md)) — peak VRAM 9.1 → 3.6 GB by compiling
the efficient kernels.

## The four iterations

| iter | data | rank | eval (tuned, no preamble) | vs base | note |
|---|---|---|---|---|---|
| 1 (pilot) | 425 | 8 | 8/11 | base 8/11 | wash (tiny, noisy eval) |
| 2 | 546, preamble-free | 8 | 24/36 | base+preamble 24/36 | **tie** — LoRA absorbs the preamble |
| 3 | 1523, academic/math | 16 | 22/49 | base+preamble 27/49 | **regressed** — format over-routing |
| 4 | 1241, rebalanced | 8 | 26/49 | base+preamble 27/49 | **tie** — regression cured |

Comparisons are tuned (NO system prompt) vs base (the full shipped preamble +
few-shot card). The tuned model carrying Myelin's behavior with *no preamble* is
the headline: it ties the prompted baseline while saving ~1,150 tokens/call.

## What iteration 3 taught us (the regression)

Adding more data — heavy on `format_note` (342 examples) at rank 16 — **collapsed
the write-vs-format decision boundary**. Probed directly (`diag.py`):

- "fix the spelling" → `format_note(strip_markdown)`  (should be `write_note`)
- "remove the second item" → `format_note(remove_bullets)`
- "clear the note" → `search_notes`

`rebalance_data.py` cut format examples 342 → 60 (the app's `detect_format_op`
gates the rest anyway) and reverted to rank 8. Iteration 4 recovered the lost
edits — confirming it was a data-balance problem, not capacity.

## The ceiling

Across iterations the tuned model **oscillates around base (22/24/26 vs 27)** — it
does not climb past it. The failures are *shared*: base WITH the full preamble and
the LoRA fail the **same** hard cases:

- surgical edits: "remove the 2nd item", "remove oranges", "add a line at the end"
- math edits: "add the formula for the mean", "shorten this abstract to one sentence"
- list conversions: numbered ↔ bullets

These are a **1 B capacity limit** — neither prompting nor fine-tuning a 1 B cracks
them. (Several eval "fails" — hello / thanks — are eval-only: the app gates tools
off for small talk, so they don't misfire in production.)

What the LoRA *does* win, preamble-free: identity (says "Myelin", never IBM/Granite),
format-op routing, and equal edit quality at zero prompt cost.

## Where it stands

- **Best 1 B artifact:** `out/granite-myelin4-Q4_K_M.gguf` (859 MB) — ties base, preamble-free.
- **Data lever is exhausted on the 1 B.** More data did not beat base (iter-3 showed
  more can hurt). To genuinely beat base on PhD-grade surgical/LaTeX edits, the
  answer is a **bigger model** (granite-4.0-h-3b), which reuses this entire pipeline.

## Open decision

1. **Ship iter-4 (1 B)** — same quality as base, ~1,150 tokens lighter/call. Efficiency win now.
2. **Train granite-4.0-h-3b** — the real ceiling-breaker; bigger download + slower, still fits the 2050.
3. **One more 1 B run** — cheapest; evidence says it ties again.

## Reproduce

`gen_data.py` (Zen, `--key-var` for parallel keys, `--resume` to accumulate) →
`merge_data.py` / `rebalance_data.py` → `train_lora.py` (preamble-free, `--rank`) →
`finalize.ps1 <adapter> <tag>` → `eval_run.ps1 <tag>` (base gets `app_preamble.txt`,
tuned gets none).
