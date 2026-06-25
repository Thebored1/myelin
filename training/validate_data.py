import json, collections, pathlib
H = pathlib.Path(__file__).parent
BS = chr(92)  # backslash, avoids any shell/heredoc escaping confusion
for fn in ("seed.jsonl", "eval.jsonl"):
    rows = [json.loads(l) for l in (H / fn).read_text(encoding="utf-8").splitlines() if l.strip()]
    lit_nl = 0   # parsed strings containing backslash+n (the bug)
    latex = 0    # records whose content has a real single backslash (LaTeX)
    for r in rows:
        blob = json.dumps(r["assistant"]) if "assistant" in r else json.dumps(r.get("check", {}))
        note = r.get("note", "")
        content = r.get("assistant", {}).get("args", {}).get("content", "") if "assistant" in r else ""
        if (BS + "n") in (note + content):
            lit_nl += 1
        if BS in content and (BS + "n") not in content:
            latex += 1
    print(f"{fn}: {len(rows)} records | literal-backslash-n bugs: {lit_nl} | records w/ LaTeX backslash: {latex}")
    if fn == "seed.jsonl":
        dist = collections.Counter(r["assistant"].get("tool", "TEXT") for r in rows)
        print("  seed tool dist:", dict(dist))
