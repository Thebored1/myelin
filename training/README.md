# Myelin LoRA training kit

Trains a LoRA for **Granite-4.0-h-1b** (the model Myelin ships by default) **entirely
on a 4 GB RTX 2050, on Windows**, with the training set generated for free via
**OpenCode Zen**. The whole pipeline is proven end-to-end and reproducible; the
adapter's *quality* is still being iterated (see [Status](#status)).

This only retrains the shipped default model. The deterministic assists
(`format_note`, `find_in_note`) + the few-shot card + the Settings toggle still
cover everything else, and power users swap their own model.

## Result so far

| | |
|---|---|
| Trains in | ~57 min (3 epochs, 162 steps, ~21 s/step) |
| Loss | 3.05 → **0.30**, token accuracy **98.3%** |
| Peak VRAM | **3.6 GB** (fits the 4 GB card) — down from 9.1 GB naive |
| Output | `out/granite-myelin-Q4_K_M.gguf` (859 MB) |
| Eval (pilot) | **8/11 vs 8/11 base — a wash.** Not shipped. |

The 9.1 → 3.6 GB drop is the whole game: Granite-4.0-h is a **Mamba-2 hybrid**, and
its efficient training scan needs CUDA/Triton kernels. Without them the naive
fallback wants a single 4.5 GB allocation and OOMs a 4 GB card. With them it fits.

## Files

```
myelin_prompt.py    system prompt + tool schemas + tools_for() — mirrors the live app
seed.jsonl          18 hand-authored gold examples (balanced)
gen_data.py         scale the seed via OpenCode Zen (stdlib, free-model rotation)
train_lora.py       4-bit QLoRA (PEFT+TRL), fast-path-friendly; also --merge
patch_causal_conv1d.py   makes the kernel source compile on Windows
build_kernels.ps1   clone + patch + compile causal-conv1d (sm_86)
runenv.ps1          dot-source: sets MSVC + CUDA + Triton env for train/eval
finalize.ps1        merge LoRA → GGUF → quantize Q4_K_M
eval.py / eval.jsonl / eval_run.ps1   base-vs-tuned scoring on a held-out set
```

## The hard part: getting Mamba-2 kernels to build on Windows + 4 GB

These are the gotchas, documented so this is reproducible (most are baked into the
scripts already):

1. **No Windows wheels** for `causal-conv1d` / `mamba-ssm`. Triton (`mamba_chunk_scan_combined`)
   needs no compile — `pip install triton-windows`. Only `causal-conv1d` must be built.
2. **No `nvcc`** and the pip `nvidia-cuda-nvcc-cu12` wheel ships `ptxas` but *not* `nvcc.exe`.
   Get a real nvcc 12.4 via **micromamba** (`cuda-nvcc` + `cuda-libraries-dev`, pinned
   `cuda-version=12.4`) — **no admin needed**.
3. **MSVC too new**: VS 18 (14.50) exceeds what CUDA 12.4 supports → `NVCC_PREPEND_FLAGS=-allow-unsupported-compiler`.
4. **Missing `cusparse.h`** (torch's CUDA headers need the math libs) → `cuda-libraries-dev`.
5. **`causal-conv1d` 1.4.0 source**: C++ alternative tokens (`and`/`or`) → `/FIiso646.h`;
   `#pragma unroll` and `#ifdef USE_ROCM` **inside macro args** → `patch_causal_conv1d.py`
   rewrites them (`_Pragma`, resolve USE_ROCM).
6. **Version lock**: `causal-conv1d 1.4.0` ↔ `mamba-ssm 2.2.4` (newer causal-conv1d changed
   the `causal_conv1d_fwd` signature).
7. **QLoRA × fused Mamba**: the fused kernel gets `out_proj` (and `lm_head`/`embed`) *raw*,
   so those must be excluded from 4-bit (`llm_int8_skip_modules`) or it can't dequantize them.
8. **DLL load order**: `import pyarrow`/`datasets` **before** torch, or Windows access-violates.

## One-time setup

```powershell
# 1. Python 3.11 venv + ML stack
uv venv training\.venv --python 3.11
uv pip install --python training\.venv\Scripts\python.exe torch --index-url https://download.pytorch.org/whl/cu124
uv pip install --python training\.venv\Scripts\python.exe "transformers>=4.48" datasets peft trl accelerate bitsandbytes triton-windows ninja einops sentencepiece huggingface_hub hf_transfer

# 2. CUDA toolchain for the kernel build (no admin) — needs MSVC / VS Build Tools installed
#    (micromamba downloaded automatically; see build_kernels.ps1 header)
micromamba create -y -p training\cudaenv -c nvidia -c conda-forge "cuda-version=12.4" cuda-nvcc cuda-cudart-dev cuda-cccl cuda-nvrtc-dev cuda-libraries-dev

# 3. Compile the kernels (clones + patches causal-conv1d automatically)
powershell -File training\build_kernels.ps1
```

## Run it

```powershell
# data (free, via OpenCode Zen — set OPENCODE_ZEN_KEY in training/.env)
python training\gen_data.py --n 800 --out training\data\train.jsonl

# base weights (HF safetensors)
huggingface-cli download ibm-granite/granite-4.0-h-1b --local-dir training\base\granite-4.0-h-1b

# train -> merge -> GGUF -> quantize -> eval
cd training; . .\runenv.ps1
.\.venv\Scripts\python.exe train_lora.py --base base\granite-4.0-h-1b --data data\train.jsonl --out out\adapter --epochs 3
powershell -File finalize.ps1
powershell -File eval_run.ps1
```

`.env` (gitignored) holds `OPENCODE_ZEN_KEY` + `OPENCODE_ZEN_BASE=https://opencode.ai/zen/v1`.
`gen_data.py` rotates the free Zen models (`mimo-v2.5-free`, `deepseek-v4-flash-free`, …)
when one rate-limits.

## Status

The **pipeline works and is reproducible** — that's the durable win. The first
425-example pilot adapter scored **8/11 vs 8/11** against base (a wash, with a small
chat-over-triggering regression), so it was **not shipped**. Next iteration: larger +
rebalanced data (more faithful-edit + chat/no-tool turns), a bigger eval set, and
evaluating against the *shipped* preamble (the few-shot card), not the lean one.
