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
import torch
from datasets import Dataset
from transformers import AutoModelForCausalLM, AutoTokenizer, BitsAndBytesConfig
from peft import LoraConfig, PeftModel

sys.path.insert(0, str(pathlib.Path(__file__).parent))
from myelin_prompt import TOOLS, build_messages


def render(tok, data_path):
    rows = []
    for line in pathlib.Path(data_path).read_text(encoding="utf-8").splitlines():
        if not line.strip():
            continue
        msgs = build_messages(json.loads(line))
        text = tok.apply_chat_template(msgs, tools=TOOLS, tokenize=False, add_generation_prompt=False)
        rows.append({"text": text})
    return Dataset.from_list(rows)


def train(a):
    from trl import SFTConfig, SFTTrainer
    tok = AutoTokenizer.from_pretrained(a.base)
    if tok.pad_token is None:
        tok.pad_token = tok.eos_token
    ds = render(tok, a.data)
    print(f"loaded {len(ds)} examples")

    bnb = BitsAndBytesConfig(load_in_4bit=True, bnb_4bit_quant_type="nf4",
                             bnb_4bit_compute_dtype=torch.bfloat16, bnb_4bit_use_double_quant=True)
    model = AutoModelForCausalLM.from_pretrained(a.base, quantization_config=bnb, device_map="auto",
                                                 trust_remote_code=True)
    lora = LoraConfig(r=a.rank, lora_alpha=a.alpha, lora_dropout=0.05, bias="none",
                      task_type="CAUSAL_LM", target_modules="all-linear")
    cfg = SFTConfig(output_dir=a.out, num_train_epochs=a.epochs, per_device_train_batch_size=a.batch,
                    gradient_accumulation_steps=a.grad_accum, learning_rate=a.lr, warmup_ratio=0.03,
                    lr_scheduler_type="cosine", logging_steps=10, save_strategy="epoch",
                    bf16=True, max_length=2048, dataset_text_field="text", packing=False)
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
    ap.add_argument("--batch", type=int, default=4)
    ap.add_argument("--grad-accum", type=int, default=4)
    ap.add_argument("--lr", type=float, default=2e-4)
    args = ap.parse_args()
    (merge if args.merge else train)(args)
