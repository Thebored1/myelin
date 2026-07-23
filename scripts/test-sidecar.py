#!/usr/bin/env python3
"""Exercise the real openharn-myelin HTTP bridge with a Markdown tool store.

This deliberately does not call Myelin's AppState. It is a black-box test of the
sidecar protocol: chat stream -> tool event -> tool-result -> continued stream.
The write/read/search tools operate on a temporary Markdown workspace so every
model can be tested without changing the user's notes.

Usage:
  python3 scripts/test-sidecar.py MODEL.gguf [OTHER_MODEL.gguf ...]
  python3 scripts/test-sidecar.py --models-dir ~/Downloads

Environment overrides:
  LLAMA_SERVER_BIN, SIDECAR_BIN, SIDECAR_PORT, LLAMA_PORT
"""
from __future__ import annotations

import argparse
import glob
import json
import os
import signal
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
from pathlib import Path

TOOLS = [
    {"type": "function", "function": {"name": "write_note", "description": "Edit the open Markdown note. Use mode replace, append, or edit.", "parameters": {"type": "object", "properties": {"content": {"type": "string"}, "mode": {"type": "string", "enum": ["replace", "append", "edit"]}, "find": {"type": "string"}}, "required": ["content"]}}},
    {"type": "function", "function": {"name": "search_notes", "description": "Search other Markdown notes in the workspace.", "parameters": {"type": "object", "properties": {"query": {"type": "string"}}, "required": ["query"]}}},
    {"type": "function", "function": {"name": "read_note", "description": "Read another Markdown note by id.", "parameters": {"type": "object", "properties": {"note_id": {"type": "string"}}, "required": ["note_id"]}}},
    {"type": "function", "function": {"name": "fetch_web_page", "description": "Fetch a public web page by URL.", "parameters": {"type": "object", "properties": {"url": {"type": "string"}}, "required": ["url"]}}},
    {"type": "function", "function": {"name": "web_search", "description": "Search the web when there is no URL.", "parameters": {"type": "object", "properties": {"query": {"type": "string"}}, "required": ["query"]}}},
    {"type": "function", "function": {"name": "format_note", "description": "Apply a Markdown formatting operation.", "parameters": {"type": "object", "properties": {"operation": {"type": "string"}}, "required": ["operation"]}}},
    {"type": "function", "function": {"name": "find_in_note", "description": "Find an exact word or phrase in the open Markdown note.", "parameters": {"type": "object", "properties": {"query": {"type": "string"}}, "required": ["query"]}}},
    {"type": "function", "function": {"name": "search_documents", "description": "Search ingested source documents.", "parameters": {"type": "object", "properties": {"query": {"type": "string"}}, "required": ["query"]}}},
]

CASES = [
    ("write_note", "Rewrite the open note as a short Markdown note about Rust, preserving useful information."),
    ("search_notes", "Search my other notes for machine learning."),
    ("read_note", "Read the note with id other and tell me what it says."),
    ("fetch_web_page", "Fetch https://example.com for me."),
    ("web_search", "Search the web for current AI news."),
    ("format_note", "Remove all bold formatting from my open note."),
    ("find_in_note", "Does the open note contain the word Rust?"),
    ("search_documents", "Search my documents for transformer architecture."),
]


def post(url: str, body: dict) -> urllib.response.addinfourl:
    data = json.dumps(body).encode()
    req = urllib.request.Request(url, data=data, headers={"Content-Type": "application/json"})
    return urllib.request.urlopen(req, timeout=30)


def wait_health(base: str) -> None:
    for _ in range(120):
        try:
            with urllib.request.urlopen(base + "/health", timeout=1) as r:
                if r.status == 200:
                    return
        except (OSError, urllib.error.URLError):
            pass
        time.sleep(0.25)
    raise RuntimeError(f"sidecar at {base} did not become healthy")


def tool_result(workspace: Path, name: str, raw: str) -> str:
    try:
        args = json.loads(raw or "{}")
    except json.JSONDecodeError:
        args = {}
    open_note = workspace / "open.md"
    if name == "write_note":
        content = str(args.get("content", ""))
        mode = args.get("mode", "replace")
        current = open_note.read_text()
        if mode == "append":
            open_note.write_text(current.rstrip() + "\n\n" + content + "\n")
        elif mode == "edit" and args.get("find") in current:
            open_note.write_text(current.replace(args["find"], content, 1))
        else:
            open_note.write_text(content)
        return json.dumps({"ok": True, "path": "open.md", "chars": len(open_note.read_text())})
    if name == "read_note":
        path = workspace / ("other.md" if args.get("note_id") == "other" else "open.md")
        return json.dumps({"id": args.get("note_id"), "content": path.read_text() if path.exists() else ""})
    if name == "search_notes":
        q = str(args.get("query", "")).lower()
        hits = [p.name for p in workspace.glob("*.md") if q in p.read_text().lower()]
        return json.dumps({"query": q, "results": hits})
    if name == "find_in_note":
        q = str(args.get("query", ""))
        return json.dumps({"query": q, "found": q.lower() in open_note.read_text().lower()})
    if name == "format_note":
        return json.dumps({"ok": True, "operation": args.get("operation"), "note": "open.md"})
    if name in ("fetch_web_page", "web_search", "search_documents"):
        return json.dumps({"ok": True, "source": name, "content": "synthetic self-test result"})
    return json.dumps({"ok": False, "error": "unknown tool"})


def run_case(sidecar: str, llama: str, model: str, workspace: Path, port: int, expected: str, prompt: str) -> bool:
    request_id = f"self-test-{expected}-{time.time_ns()}"
    body = {
        "request_id": request_id,
        "base_url": llama,
        "model": model,
        "temperature": 0.0,
        "max_turns": 3,
        "messages": [
            {"role": "system", "content": "You are a Markdown note assistant. Use the supplied tools when the user asks for an action."},
            {"role": "user", "content": prompt},
        ],
        "tools": TOOLS,
        "options": {"strict": True, "prompt_tools": True, "max_calls": 2},
    }
    seen = []
    try:
        with post(sidecar + "/v1/chat/stream", body) as response:
            event = None
            data = []
            for line in response:
                line = line.decode(errors="replace").rstrip("\r\n")
                if line.startswith("event: "):
                    event = line[7:]
                elif line.startswith("data: "):
                    data.append(line[6:])
                elif not line and event:
                    payload = json.loads("\n".join(data))
                    print(f"      SSE {event}: {json.dumps(payload, ensure_ascii=False)[:300]}")
                    if event == "tool":
                        name = payload.get("name", "")
                        seen.append(name)
                        result = tool_result(workspace, name, payload.get("arguments", "{}"))
                        print(f"      TOOL {name} args={payload.get('arguments', '{}')} -> {result[:180]}")
                        post(sidecar + "/v1/tool-result", {"request_id": request_id, "tool_call_id": payload.get("id", ""), "result": result}).close()
                    event, data = None, []
    except (OSError, urllib.error.URLError, json.JSONDecodeError) as e:
        print(f"      ERROR {e}")
        return False
    ok = expected in seen
    print(f"      {'PASS' if ok else 'FAIL'} expected {expected}; calls={seen}")
    return ok


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("models", nargs="*", help="GGUF model paths")
    parser.add_argument("--models-dir", default=None)
    parser.add_argument("--llama", default=os.environ.get("LLAMA_SERVER_BIN", "llama-server"))
    parser.add_argument("--sidecar", default=os.environ.get("SIDECAR_BIN", "openharn-myelin"))
    parser.add_argument("--sidecar-port", type=int, default=int(os.environ.get("SIDECAR_PORT", "18091")))
    parser.add_argument("--llama-port", type=int, default=int(os.environ.get("LLAMA_PORT", "18092")))
    args = parser.parse_args()
    models = [Path(p) for p in args.models]
    if args.models_dir:
        models += [Path(p) for p in glob.glob(str(Path(args.models_dir) / "*.gguf"))]
    if not models:
        models = [Path(p) for p in glob.glob(str(Path.home() / "Downloads" / "*.gguf"))]
    models = list(dict.fromkeys(p for p in models if p.exists()))
    if not models:
        parser.error("no GGUF models found; pass model paths or --models-dir")

    sidecar = subprocess.Popen([args.sidecar, "--port", str(args.sidecar_port)], stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
    sidecar_base = f"http://127.0.0.1:{args.sidecar_port}"
    total_pass = total_fail = 0
    try:
        wait_health(sidecar_base)
        for model in models:
            print(f"\n=== MODEL {model} ===")
            llama = subprocess.Popen([args.llama, "-m", str(model), "--jinja", "--ctx-size", "4096", "--port", str(args.llama_port), "--no-warmup"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            try:
                wait_health(f"http://127.0.0.1:{args.llama_port}")
                with tempfile.TemporaryDirectory(prefix="myelin-sidecar-test-") as tmp:
                    workspace = Path(tmp)
                    (workspace / "open.md").write_text("# Test Note\nRust is fast and safe.\n")
                    (workspace / "other.md").write_text("Machine learning notes.\n")
                    for expected, prompt in CASES:
                        print(f"  {expected}: {prompt}")
                        if run_case(sidecar_base, f"http://127.0.0.1:{args.llama_port}/v1", str(model), workspace, args.sidecar_port, expected, prompt):
                            total_pass += 1
                        else:
                            total_fail += 1
            finally:
                llama.send_signal(signal.SIGTERM)
                try: llama.wait(timeout=5)
                except subprocess.TimeoutExpired: llama.kill()
    finally:
        sidecar.send_signal(signal.SIGTERM)
        try: sidecar.wait(timeout=5)
        except subprocess.TimeoutExpired: sidecar.kill()
    print(f"\nRESULTS: {total_pass} passed, {total_fail} failed across {len(models)} model(s)")
    return 1 if total_fail else 0


if __name__ == "__main__":
    raise SystemExit(main())
