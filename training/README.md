# Myelin LoRA training kit

Goal: a small LoRA adapter for **Granite-4.0-h-1b** that fixes the one thing
regex and few-shot can't — **edit faithfulness** (reproduce every line that
stays, change only what was asked) — plus solid Markdown style and reliable tool
calls, *without* losing chat ability.

This trains **only the model we ship by default**. Power users swap in their own
model; the deterministic assists + the Settings toggle still cover those. The
mechanical regexes (`format_note`, `find_in_note`) stay — they own search/count
and the common cleanups; the LoRA owns the open-ended edits we can't enumerate.

```
training/
  myelin_prompt.py   # the EXACT system prompt + tool schemas + chat builder the app uses
  seed.jsonl         # hand-authored gold examples (the crux — quality > quantity)
  gen_data.py        # scale the seed into a full dataset via the Claude API
  train_lora.py      # QLoRA fine-tune (PEFT + TRL), 4-bit, all-linear targets
  eval.py            # before/after scoring against a live llama-server (no regression check)
  requirements.txt
```

## The pipeline at a glance

```
seed.jsonl ──gen_data.py──▶ data/train.jsonl ──train_lora.py──▶ out/adapter/
                                                                     │
                                              merge + convert_hf_to_gguf + quantize
                                                                     ▼
                                                          granite-myelin-Q4_K_M.gguf
                                                                     │
                                                   drop into the model-profile registry
```

## Design decisions (already made for you)

- **Train == inference.** `myelin_prompt.py` holds the real preamble and the real
  tool schemas, and builds messages exactly the way `state.rs::ask_ai_stream`
  does (`"The note currently open is titled …"` framing). If the data format
  drifts from what the app sends, the adapter won't transfer — so it's shared by
  gen/train/eval.
- **Lean preamble.** Training uses the preamble *without* the few-shot "worked
  examples" card — the LoRA learns the behavior from the **data**, not from
  in-context examples. After it works, the app can drop that card for the tuned
  model (frees ~300 tokens of context). Until then the app is unchanged.
- **Full tool set per example** (not the gated subset). Teaches the model to pick
  the right tool on its own, so the tuned model can run with **deterministic
  assists OFF** (the Settings toggle) and lighter gating.
- **Balanced data = forgetting insurance.** A LoRA is still fine-tuning — the
  adapter is active on every forward pass. The seed deliberately mixes faithful
  edits + fresh writes + non-write tool calls + plain chat + an allowed wipe, so
  the model doesn't get great at edits and forget how to call tools or talk.
- **Ship merged, not as a runtime adapter.** Merge the LoRA into the base, then
  convert to GGUF and quantize. Cleaner than `--lora` at runtime and avoids
  surprises from quantizing base + adapter separately. (Granite is a hybrid
  Mamba/transformer — `target_modules="all-linear"` covers both the attention and
  Mamba projections without naming them.)

---

# WHAT YOU NEED TO DO

I can't run a GPU or download model weights from here, so these steps are yours.
Everything else (data format, seed corpus, all four scripts) is done.

### 0. One-time: pick where you train
A **free Colab T4** or a rented A10/4090 (~$0.30/hr) is plenty for a 1B QLoRA —
minutes to ~an hour. Your RTX 2050 (4 GB) is too tight; don't fight it.

### 1. Get an Anthropic API key (for data generation)
Set `ANTHROPIC_API_KEY`. We use Claude to scale the ~15 gold examples into a few
hundred–thousand, because **data quality is the whole game**, and a strong model
writing the "after" is the cheapest way to get correct edits.

### 2. Generate the dataset
```bash
pip install -r training/requirements.txt
python training/gen_data.py --n 800 --out training/data/train.jsonl
```
Skim `data/train.jsonl` afterwards — bad examples in = bad behavior out. Delete
any where the "after" didn't faithfully reproduce the kept lines.

### 3. Download the base model in HF format (on the GPU box)
```bash
huggingface-cli download ibm-granite/granite-4.0-h-1b --local-dir base/granite-4.0-h-1b
```
(You already have the GGUF; training needs the HF safetensors.)

### 4. Train
```bash
python training/train_lora.py \
  --base base/granite-4.0-h-1b \
  --data training/data/train.jsonl \
  --out out/adapter
```
Defaults: rank 16, alpha 32, 3 epochs, lr 2e-4, 4-bit. Watch the loss; stop if it
plateaus or the eval (step 6) stops improving.

### 5. Merge + convert to GGUF
```bash
python training/train_lora.py --merge --base base/granite-4.0-h-1b --out out/adapter --merged out/merged
python llama.cpp/convert_hf_to_gguf.py out/merged --outfile out/granite-myelin.gguf
./llama.cpp/llama-quantize out/granite-myelin.gguf out/granite-myelin-Q4_K_M.gguf Q4_K_M
```

### 6. Evaluate — prove it helped and didn't regress
Run llama-server twice (base on :8120, tuned on :8121), then:
```bash
python training/eval.py --base-url http://127.0.0.1:8120 --tuned-url http://127.0.0.1:8121
```
It scores **edit faithfulness, Markdown correctness, tool-call accuracy, and
chat (no-regression)** on a held-out set. Only ship if tuned ≥ base on edits/format
**and** ties on tools/chat. If tools or chat dropped, your data was too narrow —
add more of those and retrain.

### 7. Ship it
Drop `granite-myelin-Q4_K_M.gguf` next to the app's models and add a `verified`
entry to `model-profiles.json` (role chat, supports_tools true) as the new
default. Tell me when you're here — wiring the profile + defaulting the
deterministic toggle OFF for this model is the part I can do.

---

## Pilot first (strongly recommended)
Do step 2 with `--n 150`, train 1 epoch, run eval. ~1 hour total. If edit
faithfulness moves and tools/chat hold, scale the data. If not, you've spent an
afternoon, not a month. I can read the eval output and help tune from there.
