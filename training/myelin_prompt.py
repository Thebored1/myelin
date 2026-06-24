"""The single source of truth for how training data is shaped.

It mirrors what the live app sends at inference (src-tauri/src/agent.rs preamble +
tool_specs, and state.rs::ask_ai_stream's note framing) so the LoRA transfers.
gen_data.py, train_lora.py, and eval.py all import from here.
"""

# Lean preamble: identity + rules, WITHOUT the few-shot "worked examples" card.
# The LoRA learns the behavior from data, not in-context examples. (After the
# adapter works, the app can ship this same lean preamble for the tuned model.)
SYSTEM_PROMPT = (
    "You are the assistant inside Myelin, a local notes app, powered by an open "
    "model running locally on the user's own machine. If asked what or who you "
    "are, identify yourself as Myelin's built-in AI assistant — do not claim to "
    "be proprietary or commercial software. The text of the note currently open "
    "in the editor is included in the user's message — you already have it.\n\n"
    "- To change the open note (write, rewrite, edit, format, add to, shorten, "
    "clear, etc.), call write_note with the full result, or format_note for a "
    "structural cleanup. Don't just describe the change in chat — make it with the tool.\n"
    "- Write real Markdown: a heading line starts with \"# \" (a hash then a space), "
    "\"## \" for a sub-heading; bullets start with \"- \". \"**bold**\" is NOT a heading.\n"
    "- When editing, reproduce every line that should stay and change only what was "
    "asked. Never return an empty or much-shorter note unless the user explicitly "
    "asked to clear or shorten it.\n"
    "- Use fetch_web_page only when the user gives a URL, and search_notes only when "
    "the user asks about your other notes. For greetings or general questions, just "
    "reply briefly — do not read, search, or fetch."
)

FORMAT_OPS = [
    "remove_headings", "remove_bold", "remove_italic", "remove_emphasis",
    "remove_bullets", "remove_numbering", "remove_links", "remove_images",
    "remove_code", "remove_blockquotes", "remove_strikethrough",
    "remove_horizontal_rules", "remove_blank_lines", "strip_markdown",
    "headings_to_bold", "bold_to_headings", "promote_headings", "demote_headings",
    "bullets_to_numbered", "numbered_to_bullets", "tasks_to_bullets",
    "uppercase", "lowercase", "title_case",
]

def _tool(name, desc, props, required):
    return {"type": "function", "function": {
        "name": name, "description": desc,
        "parameters": {"type": "object", "properties": props, "required": required}}}

# The full tool set the app exposes — present on EVERY training example so the
# model learns to choose, rather than relying on per-message gating.
TOOLS = [
    _tool("write_note",
          "Edit the note OPEN in the editor (never creates a separate note). Handles any "
          "request to write, draft, rewrite, edit, format, add to, shorten, or delete from "
          "the open note. mode: \"replace\" (default, whole body; empty clears it) | \"append\" | "
          "\"edit\" (replace the exact `find` text). Put the real final Markdown in content.",
          {"content": {"type": "string"},
           "mode": {"type": "string", "enum": ["replace", "append", "edit"]},
           "find": {"type": "string"}},
          ["content"]),
    _tool("format_note",
          "Apply a structural Markdown transform to the open note, done exactly in code: "
          "remove/convert headings, bold, italic, bullets, numbering, links, images, code, "
          "quotes, lists, case, etc. Prefer this for remove/strip/convert requests.",
          {"operation": {"type": "string", "enum": FORMAT_OPS}}, ["operation"]),
    _tool("find_in_note",
          "Check whether an exact word or phrase appears in the open note, and how many times.",
          {"query": {"type": "string"}}, ["query"]),
    _tool("search_notes",
          "Search the user's OTHER notes for a query. Not for the open note (already in the prompt).",
          {"query": {"type": "string"}}, ["query"]),
    _tool("read_note",
          "Read the full Markdown of ANOTHER note by id (ids come from search_notes).",
          {"note_id": {"type": "string"}}, ["note_id"]),
    _tool("fetch_web_page",
          "Fetch the text of a public web page. Use when the user gives a URL or domain.",
          {"url": {"type": "string"}}, ["url"]),
    _tool("web_search",
          "Search the web when asked to look something up and you have NO URL.",
          {"query": {"type": "string"}, "count": {"type": "integer"}}, ["query"]),
]


def tools_for(record: dict) -> list:
    """Minimal relevant tool set for one record. The full 7-tool schema is ~900
    tokens and balloons every sequence to ~1.3k — too heavy for naive-Mamba
    backprop on a 4 GB card. The app already gates tools per message, so training
    on a tight set (write_note + whatever tool this turn uses) both fits the GPU
    and mirrors inference."""
    names = ["write_note"]
    a = record["assistant"]
    if "tool" in a and a["tool"] != "write_note":
        names.append(a["tool"])
    return [t for t in TOOLS if t["function"]["name"] in names]


def build_user(note: str, instruction: str, title: str = "New note") -> str:
    """Mirror state.rs::ask_ai_stream's note framing exactly."""
    if note.strip():
        ctx = (
            f'The note currently open is titled "{title}".\n\n'
            "Here is the note's CURRENT content. When the user asks you to edit, change, "
            "format, fix, clean up, rewrite, shorten, expand, reorder, or remove part of the "
            "note, treat this as the text to modify — reproduce the parts that stay, apply the "
            "change, and pass the full result to write_note. (When you are only answering a "
            "question, use it as reference and do not echo it back verbatim.)\n"
            f"--- CURRENT NOTE ---\n{note}\n--- END CURRENT NOTE ---"
        )
    else:
        ctx = f'The note currently open is titled "{title}". It is currently empty.'
    return f"{ctx}\n\nUser request: {instruction}"


def build_messages(record: dict) -> list:
    """Compact record -> OpenAI-style chat messages (system, user, assistant).

    record = {"note": str, "instruction": str, "title"?: str,
              "assistant": {"tool": name, "args": {...}} | {"text": "..."}}
    """
    msgs = [
        {"role": "system", "content": SYSTEM_PROMPT},
        {"role": "user", "content": build_user(record["note"], record["instruction"],
                                                record.get("title", "New note"))},
    ]
    a = record["assistant"]
    if "tool" in a:
        msgs.append({"role": "assistant", "content": "", "tool_calls": [{
            "type": "function",
            "function": {"name": a["tool"], "arguments": a["args"]},
        }]})
    else:
        msgs.append({"role": "assistant", "content": a["text"]})
    return msgs
