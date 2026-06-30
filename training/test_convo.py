"""Test the conversation-array refactor: send the real message history the new
backend builds — user(search) -> assistant(tool_call) -> TOOL result -> assistant
-> user(write) — and check the model writes the actual research from the retained
tool message (not a deflection). This is the exact shape run_chat now persists.

Usage: python test_convo.py [port] [--system <preamble>]
"""
import json, sys, urllib.request

port = "8133"
pre_file = "base_preamble.txt"
a = sys.argv[1:]
if a and not a[0].startswith("--"):
    port = a[0]
if "--system" in a:
    pre_file = a[a.index("--system") + 1]
PRE = open(pre_file, encoding="utf-8").read()

SEARCH_RESULT = '''Web results for "meaning of life":

1. Meaning of life - Wikipedia: The meaning of life is the question of the significance of existence; there is no consensus. Questions include "What is the purpose of existence?".
2. Stanford Encyclopedia of Philosophy: Examines what makes a life meaningful — naturalist vs supernaturalist, objective vs subjective.
3. Psychology Today: People find meaning through relationships, purpose, contribution, growth, and for many, religion.
4. Britannica: Surveys philosophical, religious, and scientific views on life's purpose across history.
5. Existentialism (Sartre, Camus): we create our own meaning; nihilism denies inherent meaning; religious traditions locate it in the divine.'''

WRITE = [{"type": "function", "function": {"name": "write_note", "description": "Write content to the open note",
          "parameters": {"type": "object", "properties": {"content": {"type": "string"}}, "required": ["content"]}}}]
DEFLECT = ["would you like", "shall i", "i will now fetch", "i will fetch", "do you want me to", "i can fetch", "should i fetch"]


def run(n):
    # the exact real-message conversation the refactor sends on the write turn
    msgs = [
        {"role": "system", "content": PRE},
        {"role": "user", "content": 'The note open is titled "New note". It is currently empty.\n\nUser request: search the meaning of life online'},
        {"role": "assistant", "content": "", "tool_calls": [
            {"id": "c1", "type": "function", "function": {"name": "web_search", "arguments": '{"query":"meaning of life"}'}}]},
        {"role": "tool", "tool_call_id": "c1", "content": SEARCH_RESULT},
        {"role": "assistant", "content": "I searched the meaning of life online and found several perspectives."},
        {"role": "user", "content": 'The note open is titled "New note". It is currently empty.\n\nUser request: write what you understood in the note'},
    ]
    body = {"messages": msgs, "tools": WRITE, "tool_choice": "auto", "stream": False, "temperature": 0.2}
    req = urllib.request.Request(f"http://127.0.0.1:{port}/v1/chat/completions",
                                 json.dumps(body).encode(), {"Content-Type": "application/json"})
    m = json.load(urllib.request.urlopen(req, timeout=120))["choices"][0]["message"]
    tcs = m.get("tool_calls") or []
    wrote = bool(tcs) and tcs[0]["function"]["name"] == "write_note"
    content = json.loads(tcs[0]["function"]["arguments"]).get("content", "") if wrote else (m.get("content") or "")
    lc = content.lower()
    ok = wrote and len(content) > 400 and not any(p in lc for p in DEFLECT) and "meaning" in lc
    print(f"[run{n}] {'PASS' if ok else 'FAIL'} | wrote={wrote} len={len(content)} deflected={any(p in lc for p in DEFLECT)}")
    if not ok:
        print(f"   content: {content[:160]!r}")
    return ok


ok = all(run(i + 1) for i in range(3))
print("\nCONVO REFACTOR TEST:", "ALL PASS" if ok else "FAIL")
sys.exit(0 if ok else 1)
