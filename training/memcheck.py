import pyarrow, datasets, torch  # noqa
from transformers import AutoModelForCausalLM, AutoTokenizer, BitsAndBytesConfig
from peft import LoraConfig, get_peft_model, prepare_model_for_kbit_training
B = "base/granite-4.0-h-1b"
bnb = BitsAndBytesConfig(load_in_4bit=True, bnb_4bit_quant_type="nf4",
                         bnb_4bit_compute_dtype=torch.bfloat16, bnb_4bit_use_double_quant=True)
m = AutoModelForCausalLM.from_pretrained(B, quantization_config=bnb, device_map="auto")
m = prepare_model_for_kbit_training(m, use_gradient_checkpointing=True)
m.config.use_cache = False
m = get_peft_model(m, LoraConfig(r=8, lora_alpha=16, lora_dropout=0.05, bias="none",
                                 task_type="CAUSAL_LM", target_modules="all-linear"))
tok = AutoTokenizer.from_pretrained(B)
ids = tok("# Note\nThis is a test sentence to gauge memory. " * 20, return_tensors="pt").input_ids[:, :640].to("cuda")
out = m(input_ids=ids, labels=ids)
out.loss.backward()
print("fwd+bwd OK | loss %.3f | PEAK VRAM GB: %.2f" % (out.loss.item(), torch.cuda.max_memory_allocated() / 1e9))
