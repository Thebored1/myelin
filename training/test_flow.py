"""End-to-end terminal test of the research-write flow — the user's exact test:
  turn 1: "search the meaning of life online"  -> model must call web_search
  turn 2: "write what you understood"          -> model must write the real content
with the search results carried forward (what the new research buffer does).

Validates the model's behavior through the full flow against a live llama-server,
in the terminal. Usage: python test_flow.py [port] [--system <preamble_file>]
"""
import json, sys, urllib.request

port = "8133"
preamble_file = "base_preamble.txt"
a = sys.argv[1:]
if a and not a[0].startswith("--"):
    port = a[0]
if "--system" in a:
    preamble_file = a[a.index("--system") + 1]
PRE = open(preamble_file, encoding="utf-8").read()

# Deterministic stand-in for what web_search returns (so the test isn't network-flaky).
SEARCH_RESULT = '''Web results for "meaning of life":

1. Meaning of life - Wikipedia: The meaning of life is the question of the significance of existence; there is no consensus. Questions include "What is the purpose of existence?".
2. Stanford Encyclopedia of Philosophy: Examines what makes a life meaningful — naturalist vs supernaturalist, objective vs subjective.
3. Psychology Today: People find meaning through relationships, purpose, contribution, growth, and for many, religion.
4. Britannica: Surveys philosophical, religious, and scientific views on life's purpose across history.
5. Existentialism (Sartre, Camus): we create our own meaning; nihilism denies inherent meaning; religious traditions locate it in the divine.'''

TOOLS = [
    {"type": "function", "function": {"name": "web_search", "description": "Search the web",
        "parameters": {"type": "object", "properties": {"query": {"type": "string"}}, "required": ["query"]}}},
    {"type": "function", "function": {"name": "fetch_web_page", "description": "Fetch a web page",
        "parameters": {"type": "object", "properties": {"url": {"type": "string"}}, "required": ["url"]}}},
    {"type": "function", "function": {"name": "write_note", "description": "Write content to the open note",
        "parameters": {"type": "object", "properties": {"content": {"type": "string"}}, "required": ["content"]}}},
]
WRITE_ONLY = [TOOLS[2]]
DEFLECT = ["would you like", "shall i", "i will now fetch", "i will fetch", "do you want me to", "i can fetch", "should i fetch"]


def call(messages, tools):
    body = {"messages": messages, "tools": tools, "tool_choice": "auto", "stream": False, "temperature": 0.2}
    req = urllib.request.Request(f"http://127.0.0.1:{port}/v1/chat/completions",
                                 json.dumps(body).encode(), {"Content-Type": "application/json"})
    return json.load(urllib.request.urlopen(req, timeout=120))["choices"][0]["message"]


def run_once(n):
    # turn 1: search request -> expect web_search
    t1 = call([{"role": "system", "content": PRE},
               {"role": "user", "content": 'The note open is titled "New note". It is currently empty.\n\nUser request: search the meaning of life online'}],
              TOOLS)
    t1_tools = [tc["function"]["name"] for tc in (t1.get("tool_calls") or [])]
    searched = "web_search" in t1_tools
    # turn 2: write request, with the search results carried forward (the buffer)
    user2 = ('The note open is titled "New note". It is currently empty.\n\n'
             "Earlier in this conversation:\nuser: search the meaning of life online\n"
             "assistant: I searched and found several perspectives.\n\n"
             "Web research you have ALREADY gathered — use it to write the note, do not offer to fetch again:\n"
             + SEARCH_RESULT + "\n\nUser request: write what you understood in the note")
    t2 = call([{"role": "system", "content": PRE}, {"role": "user", "content": user2}], WRITE_ONLY)
    tcs = t2.get("tool_calls") or []
    wrote = bool(tcs) and tcs[0]["function"]["name"] == "write_note"
    content = json.loads(tcs[0]["function"]["arguments"]).get("content", "") if wrote else (t2.get("content") or "")
    lc = content.lower()
    real = wrote and len(content) > 400 and not any(p in lc for p in DEFLECT) and "meaning" in lc
    ok = searched and real
    print(f"[run{n}] {'PASS' if ok else 'FAIL'} | t1_called={t1_tools} searched={searched} | t2_wrote={wrote} len={len(content)} real={real}")
    if not real:
        print(f"   t2 content: {content[:160]!r}")
    return ok


ok = all(run_once(i + 1) for i in range(3))
print("\nFLOW TEST:", "ALL PASS" if ok else "FAIL")
sys.exit(0 if ok else 1)
