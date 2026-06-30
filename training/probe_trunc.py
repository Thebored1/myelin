import json, urllib.request

PORT = 39281
preamble = open('/tmp/preamble.txt').read()
TOOL = [{
    "type": "function",
    "function": {
        "name": "write_note",
        "description": "Write content to the open note",
        "parameters": {
            "type": "object",
            "properties": {"content": {"type": "string"}},
            "required": ["content"],
        },
    },
}]
USER = ("The note currently open in the editor is:\n(empty)\n\n"
        "User: write a numbered list of 25 full-sentence facts about the ocean into the note.")


def probe(msgs, label):
    body = {"messages": msgs, "tools": TOOL, "tool_choice": "auto", "stream": False, "temperature": 0}
    req = urllib.request.Request(f"http://127.0.0.1:{PORT}/v1/chat/completions",
                                 json.dumps(body).encode(), {"Content-Type": "application/json"})
    d = json.load(urllib.request.urlopen(req, timeout=120))
    c = d["choices"][0]; m = c["message"]; tcs = m.get("tool_calls") or []
    al = len(tcs[0]["function"]["arguments"]) if tcs else 0
    u = d["usage"]
    print(f"[{label}] finish={c.get('finish_reason')} compl_tokens={u['completion_tokens']} "
          f"prompt_tokens={u['prompt_tokens']} args_len={al}")
    if tcs:
        print("   tail:", repr(tcs[0]["function"]["arguments"][-90:]))
    else:
        print("   NO TOOL CALL; content_len:", len(m.get("content") or ""))


probe([{"role": "user", "content": USER}], "no-preamble")
probe([{"role": "system", "content": preamble}, {"role": "user", "content": USER}], "with-preamble")
