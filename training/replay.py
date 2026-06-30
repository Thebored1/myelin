"""Replay a captured Myelin chat request against a running llama-server and show
exactly what the model returns — assistant text + every tool call's FULL args.

Usage: python replay.py <request.json> [port]   (default port 39281)

Captured requests are written by the temporary debug logging in stream_chat.rs to
%TEMP%/myelin-req-turn<N>.json. This replays the EXACT bytes the app sent, so any
truncation in the real flow reproduces here deterministically.
"""
import json, sys, urllib.request

req_path = sys.argv[1]
port = sys.argv[2] if len(sys.argv) > 2 else "39281"
# optional: --system <file> swaps the system preamble in-place so a preamble fix
# can be tested against the captured request WITHOUT rebuilding the app.
sys_override = None
if "--system" in sys.argv:
    sys_override = open(sys.argv[sys.argv.index("--system") + 1], encoding="utf-8").read()

body = json.load(open(req_path))
body["stream"] = True
if sys_override is not None:
    for m in body["messages"]:
        if m.get("role") == "system":
            m["content"] = sys_override
            break

data = json.dumps(body).encode()
r = urllib.request.Request(f"http://127.0.0.1:{port}/v1/chat/completions",
                           data, {"Content-Type": "application/json"})

text = ""
calls = {}   # index -> {"name":.., "args":..}
finish = None
for raw in urllib.request.urlopen(r, timeout=180):
    line = raw.decode("utf-8", "ignore").strip()
    if not line.startswith("data:"):
        continue
    d = line[5:].strip()
    if d == "[DONE]":
        break
    try:
        j = json.loads(d)
    except Exception:
        continue
    ch = j["choices"][0]
    if ch.get("finish_reason"):
        finish = ch["finish_reason"]
    delta = ch["delta"]
    if delta.get("content"):
        text += delta["content"]
    for tc in (delta.get("tool_calls") or []):
        i = tc.get("index", 0)
        slot = calls.setdefault(i, {"name": "", "args": ""})
        fn = tc.get("function", {})
        if fn.get("name"):
            slot["name"] = fn["name"]
        if fn.get("arguments"):
            slot["args"] += fn["arguments"]

print(f"finish_reason: {finish}")
print(f"assistant_text ({len(text)} chars): {text[:200]!r}")
for i, c in sorted(calls.items()):
    a = c["args"]
    print(f"\n-- tool_call[{i}] {c['name']} | args_len={len(a)} --")
    # is the JSON complete/parseable?
    try:
        parsed = json.loads(a)
        content = parsed.get("content")
        print(f"   JSON: VALID | content_len={len(content) if isinstance(content,str) else 'n/a'}")
        if isinstance(content, str):
            print(f"   content tail: {content[-160:]!r}")
    except Exception as e:
        print(f"   JSON: INVALID ({e})")
        print(f"   args tail: {a[-160:]!r}")
