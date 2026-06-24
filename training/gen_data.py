"""Scale seed.jsonl into a training set using OpenCode Zen (OpenAI-compatible).

Stdlib only — runs on any Python, no pip installs. Rotates across the free Zen
models; if one rate-limits/errors, it falls back to the next.

    python training/gen_data.py --n 800 --out training/data/train.jsonl
"""
import argparse, json, os, pathlib, random, sys, time, urllib.request, urllib.error

HERE = pathlib.Path(__file__).parent
sys.path.insert(0, str(HERE))
from myelin_prompt import FORMAT_OPS

VALID_TOOLS = {"write_note", "format_note", "find_in_note", "search_notes",
               "read_note", "fetch_web_page", "web_search"}

# Rotation order: ones that actually answered first (qwen/minimax 401 for this key).
MODELS = ["mimo-v2.5-free", "deepseek-v4-flash-free", "north-mini-code-free",
          "nemotron-3-ultra-free", "qwen3.6-plus-free", "minimax-m3-free"]

# The exact arg key each tool requires — rejects schema drift (e.g. find_in_note
# coming back with {"word": ...} instead of {"query": ...}).
REQUIRED_ARG = {"find_in_note": "query", "search_notes": "query",
                "read_note": "note_id", "fetch_web_page": "url", "web_search": "query"}


def load_env():
    env = {}
    p = HERE / ".env"
    if p.exists():
        for line in p.read_text(encoding="utf-8").splitlines():
            if "=" in line and not line.strip().startswith("#"):
                k, v = line.split("=", 1)
                env[k.strip()] = v.strip()
    return (os.environ.get("OPENCODE_ZEN_KEY") or env.get("OPENCODE_ZEN_KEY"),
            os.environ.get("OPENCODE_ZEN_BASE") or env.get("OPENCODE_ZEN_BASE", "https://opencode.ai/zen/v1"))


GEN_SYSTEM = """You generate training data for a local notes assistant called Myelin.
Each record is one JSON object: {"note","title","instruction","assistant"} where
assistant is either {"tool": <name>, "args": {...}} or {"text": "..."}.

THE RULES THE MODEL MUST LEARN — every record must exemplify them:
- EDIT FAITHFULNESS: when the instruction edits PART of an existing note, the
  write_note `content` must REPRODUCE every line that stays, unchanged, and apply
  ONLY the requested change. Never drop or rewrite untouched lines. Never return an
  empty/much-shorter note unless the user asked to clear or shorten it.
- MARKDOWN: headings are "# "/"## " lines (never **bold** as a heading); bullets "- ".
- TOOL CHOICE: edits/writes -> write_note; remove/strip/convert structure ->
  format_note with one operation of: %s; "does word X appear" -> find_in_note;
  other notes -> search_notes; a URL -> fetch_web_page; a lookup with no URL ->
  web_search; a greeting or a question you can just answer -> {"text": ...} (NO tool).

Produce a DIVERSE mix across ALL those categories and across note topics, lengths,
and phrasings. Output ONLY a JSON array of records, no prose, no markdown fences.""" % ", ".join(FORMAT_OPS)


def chat(base, key, model, user, max_tokens=4096, timeout=180):
    body = json.dumps({"model": model, "messages": [
        {"role": "system", "content": GEN_SYSTEM}, {"role": "user", "content": user}],
        "max_tokens": max_tokens, "temperature": 0.9, "stream": False}).encode()
    req = urllib.request.Request(base + "/chat/completions", body, {
        "Authorization": f"Bearer {key}", "Content-Type": "application/json",
        # opencode.ai sits behind Cloudflare, which 403s the default python-urllib UA.
        "User-Agent": "curl/8.4.0", "Accept": "application/json"})
    with urllib.request.urlopen(req, timeout=timeout) as r:
        msg = json.loads(r.read())["choices"][0]["message"]
    return msg.get("content") or ""


def extract(text):
    i, j = text.find("["), text.rfind("]")
    if i < 0 or j < 0:
        return []
    try:
        return json.loads(text[i:j + 1])
    except json.JSONDecodeError:
        return []


def valid(r):
    if not isinstance(r, dict) or not all(k in r for k in ("note", "instruction", "assistant")):
        return False
    a = r["assistant"]
    if not isinstance(a, dict):
        return False
    if "text" in a:
        return isinstance(a["text"], str) and a["text"].strip() != ""
    if a.get("tool") not in VALID_TOOLS:
        return False
    args = a.get("args", {})
    if a["tool"] == "format_note" and args.get("operation") not in FORMAT_OPS:
        return False
    if a["tool"] in REQUIRED_ARG and not str(args.get(REQUIRED_ARG[a["tool"]], "")).strip():
        return False
    if a["tool"] == "write_note":
        if "content" not in args:
            return False
        # Reject the wipe failure mode: emptying a non-empty note unless asked.
        if r["note"].strip() and not args["content"].strip():
            instr = r["instruction"].lower()
            if not any(w in instr for w in ("clear", "empty", "delete everything", "wipe", "erase", "blank")):
                return False
    return True


def load_seed():
    return [json.loads(l) for l in (HERE / "seed.jsonl").read_text(encoding="utf-8").splitlines() if l.strip()]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--n", type=int, default=800)
    ap.add_argument("--batch", type=int, default=12)
    ap.add_argument("--out", default=str(HERE / "data" / "train.jsonl"))
    a = ap.parse_args()

    key, base = load_env()
    if not key:
        sys.exit("set OPENCODE_ZEN_KEY (training/.env)")
    seed = load_seed()
    out_path = pathlib.Path(a.out)
    out_path.parent.mkdir(parents=True, exist_ok=True)

    records = list(seed)
    seen = {json.dumps([r["note"], r["instruction"]], sort_keys=True) for r in seed}
    mi, fails = 0, 0
    while len(records) - len(seed) < a.n:
        model = MODELS[mi % len(MODELS)]
        anchors = random.sample(seed, min(6, len(seed)))
        user = ("Examples:\n" + "\n".join(json.dumps(x, ensure_ascii=False) for x in anchors)
                + f"\n\nOutput a JSON array of {a.batch} NEW, different records.")
        try:
            batch = extract(chat(base, key, model, user))
        except (urllib.error.HTTPError, urllib.error.URLError, TimeoutError) as e:
            print(f"  [{model}] error: {e} -> rotating"); mi += 1; fails += 1
            if fails > len(MODELS) * 3:
                sys.exit("all models failing; stopping")
            time.sleep(2); continue
        added = 0
        for r in batch:
            if not valid(r):
                continue
            k = json.dumps([r["note"], r["instruction"]], sort_keys=True)
            if k in seen:
                continue
            seen.add(k); records.append(r); added += 1
        print(f"  [{model}] +{added}  ({len(records)-len(seed)}/{a.n})")
        if added == 0:
            mi += 1  # unproductive model -> rotate
        # incremental save so a crash never loses progress
        with out_path.open("w", encoding="utf-8") as f:
            for r in records:
                f.write(json.dumps(r, ensure_ascii=False) + "\n")
    print(f"done: {len(records)} records ({len(seed)} seed + {len(records)-len(seed)} gen) -> {out_path}")


if __name__ == "__main__":
    main()
