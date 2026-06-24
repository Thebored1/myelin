"""Scale seed.jsonl into a full training set using the Claude API.

Claude writes new (note, instruction, assistant) records in the same compact
schema, anchored on the hand-authored seed so the category balance and the
faithful-edit invariant carry over. We validate structure here; you still skim
the output (bad "after" edits poison the model).

    export ANTHROPIC_API_KEY=...
    python training/gen_data.py --n 800 --out training/data/train.jsonl
"""
import argparse, json, os, pathlib, random, sys

try:
    from anthropic import Anthropic
except ImportError:
    sys.exit("pip install -r training/requirements.txt first")

from myelin_prompt import FORMAT_OPS

HERE = pathlib.Path(__file__).parent
VALID_TOOLS = {"write_note", "format_note", "find_in_note", "search_notes",
               "read_note", "fetch_web_page", "web_search"}

GEN_SYSTEM = """You generate training data for a local notes assistant called Myelin.
Each record is one JSON object: {"note","title","instruction","assistant"} where
assistant is either {"tool": <name>, "args": {...}} or {"text": "..."}.

THE RULES THE MODEL MUST LEARN — every record must exemplify them:
- EDIT FAITHFULNESS: when the instruction edits PART of an existing note, the
  write_note `content` must REPRODUCE every line that stays, unchanged, and apply
  ONLY the requested change. Never drop or rewrite untouched lines. Never return
  an empty/much-shorter note unless the user asked to clear or shorten it.
- MARKDOWN: headings are "# "/"## " lines (never **bold** as a heading); bullets "- ".
- TOOL CHOICE: edits/writes -> write_note; remove/strip/convert structure ->
  format_note with one of these operations: %s; "does word X appear" -> find_in_note;
  other notes -> search_notes; a URL -> fetch_web_page; a lookup with no URL ->
  web_search; a greeting or a question you can just answer -> {"text": ...} (NO tool).

Produce a DIVERSE mix across all those categories and across note topics, lengths,
and instruction phrasings. Vary realistic notes (lists, prose, mixed). Output ONLY
a JSON array of records, nothing else.""" % ", ".join(FORMAT_OPS)


def load_seed():
    return [json.loads(l) for l in (HERE / "seed.jsonl").read_text(encoding="utf-8").splitlines() if l.strip()]


def valid(r):
    if not all(k in r for k in ("note", "instruction", "assistant")):
        return False
    a = r["assistant"]
    if "text" in a:
        return isinstance(a["text"], str) and a["text"].strip() != ""
    if a.get("tool") not in VALID_TOOLS:
        return False
    args = a.get("args", {})
    if a["tool"] == "format_note" and args.get("operation") not in FORMAT_OPS:
        return False
    if a["tool"] == "write_note" and "content" not in args:
        return False
    return True


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--n", type=int, default=800, help="how many generated records to target")
    ap.add_argument("--batch", type=int, default=20)
    ap.add_argument("--model", default="claude-sonnet-4-6")
    ap.add_argument("--out", default=str(HERE / "data" / "train.jsonl"))
    args = ap.parse_args()

    client = Anthropic()
    seed = load_seed()
    out_path = pathlib.Path(args.out)
    out_path.parent.mkdir(parents=True, exist_ok=True)

    records, seen = list(seed), {json.dumps([r["note"], r["instruction"]], sort_keys=True) for r in seed}
    while len(records) - len(seed) < args.n:
        anchors = random.sample(seed, min(6, len(seed)))
        user = ("Here are example records:\n" + "\n".join(json.dumps(a, ensure_ascii=False) for a in anchors)
                + f"\n\nNow output a JSON array of {args.batch} NEW, different records.")
        msg = client.messages.create(model=args.model, max_tokens=4096,
                                      system=GEN_SYSTEM, messages=[{"role": "user", "content": user}])
        text = msg.content[0].text.strip()
        text = text[text.find("["): text.rfind("]") + 1]
        try:
            batch = json.loads(text)
        except json.JSONDecodeError:
            print("  (skipped an unparseable batch)"); continue
        added = 0
        for r in batch:
            if not valid(r):
                continue
            key = json.dumps([r["note"], r["instruction"]], sort_keys=True)
            if key in seen:
                continue
            seen.add(key); records.append(r); added += 1
        print(f"  +{added}  ({len(records) - len(seed)}/{args.n})")

    with out_path.open("w", encoding="utf-8") as f:
        for r in records:
            f.write(json.dumps(r, ensure_ascii=False) + "\n")
    print(f"wrote {len(records)} records ({len(seed)} seed + {len(records)-len(seed)} generated) -> {out_path}")
    print("NOW SKIM IT: delete any write_note where the 'after' dropped lines it should have kept.")


if __name__ == "__main__":
    main()
