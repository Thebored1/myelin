"""Combine the iter-3 generated set with the iter-2 set (and seed) into one
training corpus: dedup by (note, instruction) and re-apply the faithfulness
validator so older, unfiltered edits get dropped. Output: data/train_combined.jsonl"""
import json, pathlib, sys, collections
sys.path.insert(0, str(pathlib.Path(__file__).parent))
from gen_data import valid

H = pathlib.Path(__file__).parent
out = H / "data" / "train_combined.jsonl"
sources = ["data/train.jsonl", "data/train_k2.jsonl", "data/train_iter2.jsonl"]  # iter3 (both keys) + iter2

seen, recs, dropped = set(), [], 0
for fn in sources:
    p = H / fn
    if not p.exists():
        print(f"(skip missing {fn})")
        continue
    n0 = len(recs)
    for line in p.read_text(encoding="utf-8").splitlines():
        if not line.strip():
            continue
        r = json.loads(line)
        k = json.dumps([r["note"], r["instruction"]], sort_keys=True)
        if k in seen:
            continue
        if not valid(r):
            dropped += 1
            continue
        seen.add(k); recs.append(r)
    print(f"{fn}: +{len(recs) - n0} kept")

out.write_text("\n".join(json.dumps(r, ensure_ascii=False) for r in recs) + "\n", encoding="utf-8")
dist = collections.Counter(r["assistant"].get("tool", "TEXT") for r in recs)
print(f"combined {len(recs)} records (dropped {dropped} invalid/unfaithful) -> {out}")
print("dist:", dict(dist))
