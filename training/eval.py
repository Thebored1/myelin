"""Score base vs tuned model on held-out cases against a running llama-server.

Start two servers (merged-tuned GGUF and stock GGUF), then:
    llama-server -m stock.gguf  --port 8120 --jinja &
    llama-server -m tuned.gguf  --port 8121 --jinja &
    python training/eval.py --base-url http://127.0.0.1:8120 --tuned-url http://127.0.0.1:8121

Checks per case: right tool + args, edit faithfulness (kept lines present,
removed text absent), Markdown style, and chat cases that must NOT call a tool.
Ship only if tuned >= base on edits/format AND ties on tools/chat.
"""
import argparse, json, pathlib, sys, urllib.request

sys.path.insert(0, str(pathlib.Path(__file__).parent))
from myelin_prompt import SYSTEM_PROMPT, TOOLS, build_user

HERE = pathlib.Path(__file__).parent


def ask(base_url, note, title, instruction):
    payload = {"messages": [{"role": "system", "content": SYSTEM_PROMPT},
                            {"role": "user", "content": build_user(note, instruction, title)}],
               "tools": TOOLS, "temperature": 0.2, "stream": False}
    req = urllib.request.Request(base_url + "/v1/chat/completions",
                                 json.dumps(payload).encode(), {"Content-Type": "application/json"})
    m = json.loads(urllib.request.urlopen(req, timeout=120).read())["choices"][0]["message"]
    tc = m.get("tool_calls")
    if tc:
        fn = tc[0]["function"]
        args = fn["arguments"]
        if isinstance(args, str):
            try: args = json.loads(args)
            except json.JSONDecodeError: args = {}
        return {"tool": fn["name"], "args": args}
    return {"text": m.get("content", "")}


def grade(resp, check):
    if check.get("no_tool"):
        return "text" in resp
    if "tool" in check and resp.get("tool") != check["tool"]:
        return False
    for k, v in check.get("args_contains", {}).items():
        if str(resp.get("args", {}).get(k, "")).strip() != str(v):
            return False
    content = resp.get("args", {}).get("content", "")
    if any(s not in content for s in check.get("content_has", [])):
        return False
    if any(s in content for s in check.get("content_lacks", [])):
        return False
    return True


def run(url, cases):
    passed = []
    for c in cases:
        try:
            ok = grade(ask(url, c["note"], c.get("title", "New note"), c["instruction"]), c["check"])
        except Exception as e:
            print(f"  ! {c['instruction'][:40]}: {e}"); ok = False
        passed.append(ok)
    return passed


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--base-url")
    ap.add_argument("--tuned-url", required=True)
    ap.add_argument("--cases", default=str(HERE / "eval.jsonl"))
    a = ap.parse_args()
    cases = [json.loads(l) for l in pathlib.Path(a.cases).read_text(encoding="utf-8").splitlines() if l.strip()]

    tuned = run(a.tuned_url, cases)
    base = run(a.base_url, cases) if a.base_url else None
    print(f"\n{'instruction':42} {'base' if base else '':>6} tuned")
    for i, c in enumerate(cases):
        b = ("PASS" if base[i] else "fail") if base else ""
        print(f"{c['instruction'][:40]:42} {b:>6} {'PASS' if tuned[i] else 'fail'}")
    print("-" * 60)
    if base:
        print(f"{'TOTAL':42} {sum(base):>4}/{len(cases)} {sum(tuned):>4}/{len(cases)}")
    else:
        print(f"{'TOTAL':42} {sum(tuned):>4}/{len(cases)}")


if __name__ == "__main__":
    main()
