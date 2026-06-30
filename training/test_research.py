"""Terminal test for the research-write fix: simulate the FIXED prompt (recent web
research injected into context) and check the model writes the actual content into
the note instead of deflecting ("would you like me to fetch the full text?").

Runs against the live llama-server, so it validates the fix BEFORE rebuilding.
Usage: python test_research.py [port] [--system <preamble_file>]
"""
import json, sys, urllib.request

port = "39281"
preamble_file = "base_preamble.txt"
args = sys.argv[1:]
if args and not args[0].startswith("--"):
    port = args[0]
if "--system" in args:
    preamble_file = args[args.index("--system") + 1]
preamble = open(preamble_file, encoding="utf-8").read()

RESEARCH = '''Web results for "meaning of life":

1. Meaning of life - Wikipedia: The meaning of life is the question of the significance of living or existence in general. There is no consensus; questions include "What is the purpose of existence?" and "Why are we here?".
2. The Meaning of Life (Stanford Encyclopedia of Philosophy): Examines what makes a life meaningful — naturalist vs supernaturalist views, and whether meaning is objective or subjective.
3. Psychology Today: Many people find meaning through relationships, purpose, contribution, and personal growth; others through religion or spirituality.
4. Britannica: Surveys philosophical, religious, and scientific perspectives on life's purpose across history.
5. Philosophical traditions: Existentialists (Sartre, Camus) hold that we create our own meaning; nihilists deny inherent meaning; many religious traditions locate meaning in the divine.'''

user = (
    'The note currently open is titled "New note 4". It is currently empty.\n\n'
    "Earlier in this conversation:\n"
    "user: search the meaning of life online\n"
    "assistant: I searched the meaning of life online and found several perspectives.\n\n"
    "Web research you have ALREADY gathered in this conversation — use it directly to "
    "write the note; do NOT offer to fetch or search again, you already have this:\n"
    + RESEARCH
    + "\n\nUser request: write what you understood in the note"
)

TOOL = [{
    "type": "function",
    "function": {
        "name": "write_note",
        "description": "Write content to the open note",
        "parameters": {"type": "object", "properties": {"content": {"type": "string"}}, "required": ["content"]},
    },
}]

DEFLECTION = ["would you like", "shall i", "i will now fetch", "i will fetch", "do you want me to",
              "let me know if", "i can fetch", "should i fetch"]


def run(label):
    body = {"messages": [{"role": "system", "content": preamble}, {"role": "user", "content": user}],
            "tools": TOOL, "tool_choice": "auto", "stream": False, "temperature": 0.2}
    req = urllib.request.Request(f"http://127.0.0.1:{port}/v1/chat/completions",
                                 json.dumps(body).encode(), {"Content-Type": "application/json"})
    d = json.load(urllib.request.urlopen(req, timeout=120))
    m = d["choices"][0]["message"]
    tcs = m.get("tool_calls") or []
    if not tcs:
        print(f"[{label}] FAIL — no write_note call (text: {(m.get('content') or '')[:80]!r})")
        return False
    content = json.loads(tcs[0]["function"]["arguments"]).get("content", "")
    lc = content.lower()
    deflected = any(p in lc for p in DEFLECTION)
    substantial = len(content) > 400
    on_topic = "meaning" in lc and ("wikipedia" in lc or "stanford" in lc or "existential" in lc or "philosoph" in lc)
    ok = substantial and not deflected and on_topic
    print(f"[{label}] {'PASS' if ok else 'FAIL'} — len={len(content)} deflected={deflected} on_topic={on_topic}")
    print(f"   head: {content[:120]!r}")
    print(f"   tail: {content[-90:]!r}")
    return ok


ok = all(run(f"run{i+1}") for i in range(3))
print("\nRESULT:", "ALL PASS" if ok else "FAIL")
sys.exit(0 if ok else 1)
