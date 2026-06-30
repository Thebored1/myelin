"""Iteration 4: fix the format_note over-routing seen in iter-3 by rebalancing.

iter-3 collapsed write-vs-format because format_note examples (often phrased like
edits: "remove the X") outnumbered the signal. The app's detect_format_op already
GATES format_note (only offered for clear all/every/convert requests), so the LoRA
needs only a FEW unambiguous format examples — not 342. Keep every write-edit /
identity / chat / other-tool record; keep only format_note examples whose
instruction is unambiguously whole-document (all/every/entire/strip/convert)."""
import json, pathlib, collections, random

H = pathlib.Path(__file__).parent
rows = [json.loads(l) for l in (H / "data" / "train_combined.jsonl").read_text(encoding="utf-8").splitlines() if l.strip()]
random.seed(0)

UNAMBIG = ("all ", "every", "entire", "strip", "convert", "whole", " into ", "everything")
keep, fmt_kept, fmt_drop = [], 0, 0
fmt_unambig = []
for r in rows:
    if r["assistant"].get("tool") == "format_note":
        instr = r["instruction"].lower()
        if any(w in instr for w in UNAMBIG):
            fmt_unambig.append(r)
        else:
            fmt_drop += 1
    else:
        keep.append(r)

# cap the format set so it can't dominate (write:format >> 1)
random.shuffle(fmt_unambig)
fmt_final = fmt_unambig[:60]
fmt_drop += len(fmt_unambig) - len(fmt_final)
keep.extend(fmt_final)
random.shuffle(keep)

out = H / "data" / "train_rebal.jsonl"
out.write_text("\n".join(json.dumps(r, ensure_ascii=False) for r in keep) + "\n", encoding="utf-8")
dist = collections.Counter(r["assistant"].get("tool", "TEXT") for r in keep)
print(f"rebalanced {len(keep)} records (dropped {fmt_drop} ambiguous/excess format_note) -> {out}")
print("dist:", dict(dist))
