"""QLoRA fine-tune Granite-4.0-h-1b on the Myelin dataset, or merge the adapter.

Train:
    python training/train_lora.py --base base/granite-4.0-h-1b \
        --data training/data/train.jsonl --out out/adapter
Merge (for GGUF conversion):
    python training/train_lora.py --merge --base base/granite-4.0-h-1b \
        --out out/adapter --merged out/merged

Run on a T4/A10/4090. `target_modules="all-linear"` covers both the transformer
attention and the Mamba projections in Granite's hybrid blocks, so we don't have
to name them.
"""
import argparse, json, pathlib, sys
# Import Arrow/datasets BEFORE torch: on Windows torch loads a native lib that
# clashes with pyarrow's if pyarrow comes second -> access-violation segfault.
import pyarrow  # noqa: F401
from datasets import Dataset
import torch
from transformers import AutoModelForCausalLM, AutoTokenizer, BitsAndBytesConfig
from peft import LoraConfig, PeftModel, prepare_model_for_kbit_training

sys.path.insert(0, str(pathlib.Path(__file__).parent))
from myelin_prompt import build_messages, tools_for


def render(tok, data_path):
    rows = []
    for line in pathlib.Path(data_path).read_text(encoding="utf-8").splitlines():
        if not line.strip():
            continue
        rec = json.loads(line)
        text = tok.apply_chat_template(build_messages(rec), tools=tools_for(rec),
                                       tokenize=False, add_generation_prompt=False)
        rows.append({"text": text})
    return Dataset.from_list(rows)


def train(a):
    from trl import SFTConfig, SFTTrainer
    tok = AutoTokenizer.from_pretrained(a.base)
    if tok.pad_token is None:
        tok.pad_token = tok.eos_token
    ds = render(tok, a.data)
    print(f"loaded {len(ds)} examples")

    # Skip the Mamba mixer's out_proj from 4-bit: the fused Mamba-2 kernel receives
    # that weight RAW (not via a Linear4bit forward), so it must stay a real tensor.
    bnb = BitsAndBytesConfig(load_in_4bit=True, bnb_4bit_quant_type="nf4",
                             bnb_4bit_compute_dtype=torch.bfloat16, bnb_4bit_use_double_quant=True,
                             llm_int8_skip_modules=["out_proj", "lm_head", "embed_tokens"])
    model = AutoModelForCausalLM.from_pretrained(a.base, quantization_config=bnb, device_map="auto",
                                                 trust_remote_code=True)
    model = prepare_model_for_kbit_training(model, use_gradient_checkpointing=True)
    model.config.use_cache = False
    lora = LoraConfig(r=a.rank, lora_alpha=a.alpha, lora_dropout=0.05, bias="none",
                      task_type="CAUSAL_LM", target_modules="all-linear")
    # Frugal settings for a 4 GB card: batch 1, short seqs (our notes are small),
    # checkpointed activations, paged 8-bit optimizer.
    cfg = SFTConfig(output_dir=a.out, num_train_epochs=a.epochs, per_device_train_batch_size=a.batch,
                    gradient_accumulation_steps=a.grad_accum, learning_rate=a.lr, warmup_ratio=0.03,
                    lr_scheduler_type="cosine", logging_steps=5, save_strategy="epoch",
                    bf16=True, max_length=a.max_len, dataset_text_field="text", packing=False,
                    gradient_checkpointing=True, gradient_checkpointing_kwargs={"use_reentrant": False},
                    optim="paged_adamw_8bit", report_to="none",
                    max_steps=a.max_steps if a.max_steps > 0 else -1)
    trainer = SFTTrainer(model=model, args=cfg, train_dataset=ds, peft_config=lora, processing_class=tok)
    trainer.train()
    trainer.save_model(a.out)
    tok.save_pretrained(a.out)
    print(f"adapter saved -> {a.out}")


def merge(a):
    tok = AutoTokenizer.from_pretrained(a.base)
    base = AutoModelForCausalLM.from_pretrained(a.base, torch_dtype=torch.bfloat16,
                                                device_map="cpu", trust_remote_code=True)
    merged = PeftModel.from_pretrained(base, a.out).merge_and_unload()
    merged.save_pretrained(a.merged)
    tok.save_pretrained(a.merged)
    print(f"merged model saved -> {a.merged}\nNow: convert_hf_to_gguf.py {a.merged} && llama-quantize ... Q4_K_M")


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("--base", required=True)
    ap.add_argument("--out", default="out/adapter")
    ap.add_argument("--data", default="training/data/train.jsonl")
    ap.add_argument("--merge", action="store_true")
    ap.add_argument("--merged", default="out/merged")
    ap.add_argument("--rank", type=int, default=16)
    ap.add_argument("--alpha", type=int, default=32)
    ap.add_argument("--epochs", type=float, default=3)
    ap.add_argument("--batch", type=int, default=1)
    ap.add_argument("--grad-accum", type=int, default=8)
    ap.add_argument("--lr", type=float, default=2e-4)
    ap.add_argument("--max-len", type=int, default=1024)
    ap.add_argument("--max-steps", type=int, default=0, help=">0 caps steps (smoke test)")
    args = ap.parse_args()
    (merge if args.merge else train)(args)
