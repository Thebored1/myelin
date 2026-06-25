"""Probe a running model on specific failing edit cases — print the RAW response
(tool call or text) so we can see HOW it fails (mis-route vs unfaithful edit)."""
import json, sys, urllib.request
sys.path.insert(0, ".")
from myelin_prompt import build_user, TOOLS

URL = sys.argv[1] if len(sys.argv) > 1 else "http://127.0.0.1:8121"
CASES = [
    ("# Cats\nCats are independant animals.\nThey sleep a lot.", "fix the spelling"),
    ("# Plan\nStep one: research.\nStep two: build.\nStep three: ship.", "change step two to say design"),
    ("# L\n- a\n- b\n- c\n- d", "remove the second item"),
    ("# Old\njunk i don't want", "clear the note"),
]
for note, instr in CASES:
    payload = {"messages": [{"role": "user", "content": build_user(note, "Note", instr)}],
               "tools": TOOLS, "temperature": 0.2, "stream": False}
    req = urllib.request.Request(URL + "/v1/chat/completions", json.dumps(payload).encode(),
                                 {"Content-Type": "application/json"})
    m = json.loads(urllib.request.urlopen(req, timeout=120).read())["choices"][0]["message"]
    tc = m.get("tool_calls")
    print("=" * 50)
    print("INSTR:", instr)
    if tc:
        fn = tc[0]["function"]
        print("TOOL:", fn["name"])
        print("ARGS:", str(fn["arguments"])[:300])
    else:
        print("TEXT:", (m.get("content") or "")[:300])
