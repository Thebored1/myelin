import json, urllib.request

PORT = 39281
preamble = open('/tmp/preamble.txt').read()
def fn(name, desc, props, req):
    return {"type": "function", "function": {"name": name, "description": desc,
            "parameters": {"type": "object", "properties": props, "required": req}}}

# The realistic toolset the app offers for a write+search message on an open note.
TOOL = [
    fn("write_note", "Write content to the open note",
       {"content": {"type": "string"}, "mode": {"type": "string"}, "find": {"type": "string"}}, ["content"]),
    fn("format_note", "Apply a structural format operation to the open note",
       {"operation": {"type": "string"}}, ["operation"]),
    fn("web_search", "Search the web", {"query": {"type": "string"}}, ["query"]),
    fn("fetch_web_page", "Fetch and read a web page", {"url": {"type": "string"}}, ["url"]),
    fn("search_notes", "Search the user's other notes", {"query": {"type": "string"}}, ["query"]),
    fn("read_note", "Read another note by id", {"note_id": {"type": "string"}}, ["note_id"]),
    fn("find_in_note", "Find a word in the open note", {"query": {"type": "string"}}, ["query"]),
]

SEARCH = '''Web results for "meaning of life":

1. Meaning of life - Wikipedia
   https://en.wikipedia.org/wiki/Meaning_of_life
   The meaning of life is the concept of an individual's life, or existence in general, having an inherent significance or a philosophical point. There is no consensus on the specifics of such a concept or whether it even exists.
2. The Meaning of Life (Stanford Encyclopedia of Philosophy)
   https://plato.stanford.edu/entries/life-meaning/
   Discusses the common questions surrounding the meaning of life, such as what it means for life to be meaningful and whether life is in fact meaningful.
3. What Is the Meaning of Life? - Psychology Today
   https://www.psychologytoday.com/meaning
   Many people believe humankind is a creation of a higher power, while others see meaning as something each person constructs for themselves.
4. Meaning of Life | Philopedia
   https://philopedia.org/meaning-of-life
   Offers an exploration of philosophical theories on the purpose of human existence, from nihilism to existentialism to religious accounts.
5. The Meaning of Life: A Very Short Introduction
   https://example.org/very-short
   A concise overview of how thinkers across history have approached the question of life's purpose and significance.

To read a result in full, call fetch_web_page with its URL.'''

msgs = [
    {"role": "system", "content": preamble},
    {"role": "user", "content": "The note currently open in the editor is:\n(empty)\n\nUser: search the meaning of life online and write all of the results into this note."},
    {"role": "assistant", "content": "", "tool_calls": [
        {"id": "c1", "type": "function", "function": {"name": "web_search", "arguments": '{"query":"meaning of life"}'}}]},
    {"role": "tool", "tool_call_id": "c1", "content": SEARCH},
    {"role": "assistant", "content": (
        "I have searched for the meaning of life online. Here are some of the top results:\n\n"
        "1. **Meaning of life - Wikipedia**: The meaning of life is the concept of an individual's life having an inherent significance; there is no consensus on the specifics.\n"
        "2. **The Meaning of Life (Stanford Encyclopedia of Philosophy)**: Discusses what it means for life to be meaningful and whether life is in fact meaningful.\n"
        "3. **What Is the Meaning of Life? - Psychology Today**: Many see meaning as a creation of a higher power; others construct it themselves.\n"
        "4. **Meaning of Life | Philopedia**: Explores philosophical theories on the purpose of human existence, from nihilism to existentialism.\n"
        "5. **The Meaning of Life: A Very Short Introduction**: A concise overview of how thinkers across history approached the question.")},
    {"role": "user", "content": "Write this in the note with proper formatting."},
]

body = {"messages": msgs, "tools": TOOL, "tool_choice": "auto", "stream": True, "temperature": 0}
req = urllib.request.Request(f"http://127.0.0.1:{PORT}/v1/chat/completions",
                             json.dumps(body).encode(), {"Content-Type": "application/json"})
args = ""; fr = None; done = False; nchunks = 0
for raw in urllib.request.urlopen(req, timeout=120):
    line = raw.decode("utf-8", "ignore").strip()
    if not line.startswith("data:"):
        continue
    dd = line[5:].strip()
    if dd == "[DONE]":
        done = True
        break
    try:
        j = json.loads(dd)
    except Exception:
        continue
    ch = j["choices"][0]; nchunks += 1
    if ch.get("finish_reason"):
        fr = ch["finish_reason"]
    for tc in (ch["delta"].get("tool_calls") or []):
        a = tc.get("function", {}).get("arguments")
        if a:
            args += a
print(f"STREAM: chunks={nchunks} DONE={done} finish_reason={fr}")
print("args_len:", len(args))
print("tail:", repr(args[-110:]))
