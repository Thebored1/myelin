#!/usr/bin/env python3
"""Benchmark Myelin's sidecar path across local GGUF models.

This is intentionally a black-box benchmark of the same sidecar protocol used
by the desktop app. It measures direct Chat, repeated-prefix cache reuse, and
targeted/tool requests without modifying user notes.
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import os
import re
import signal
import subprocess
import tempfile
import time
import urllib.error
import urllib.request
from pathlib import Path


def load_test_helpers():
    path = Path(__file__).with_name("test-sidecar.py")
    spec = importlib.util.spec_from_file_location("myelin_sidecar_test", path)
    if spec is None or spec.loader is None:
        raise RuntimeError("could not load test-sidecar helpers")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


HELPERS = load_test_helpers()
TOOLS = HELPERS.TOOLS
tool_result = HELPERS.tool_result


def post_json(url: str, body: dict):
    data = json.dumps(body).encode()
    request = urllib.request.Request(
        url, data=data, headers={"Content-Type": "application/json"}
    )
    return urllib.request.urlopen(request, timeout=45)


def wait_health(url: str, timeout: float = 90.0) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            with urllib.request.urlopen(url + "/health", timeout=1) as response:
                if response.status == 200:
                    return
        except (OSError, urllib.error.URLError):
            time.sleep(0.25)
    raise RuntimeError(f"service did not become healthy: {url}")


def parse_elapsed(message: str) -> float | None:
    match = re.search(r"elapsed_ms=(\d+)", message)
    return float(match.group(1)) if match else None


def run_request(
    sidecar_base: str,
    llama_base: str,
    model: str,
    name: str,
    messages: list[dict],
    tools: list[dict],
    options: dict,
    workspace: Path,
    max_tokens: int,
) -> dict:
    request_id = f"benchmark-{name}-{time.time_ns()}"
    body = {
        "request_id": request_id,
        "base_url": llama_base,
        "model": model,
        "temperature": 0.0,
        "max_tokens": max_tokens,
        "max_turns": 3,
        "messages": messages,
        "tools": tools,
        "options": options,
        "session": {"slot_id": 0, "epoch": 1, "pass_kind": "benchmark"},
    }
    started = time.perf_counter()
    first_delta = None
    response_headers = None
    usage = None
    calls: list[str] = []
    content = ""
    error = None
    try:
        with post_json(sidecar_base + "/v1/chat/stream", body) as response:
            event = None
            data: list[str] = []
            for line in response:
                line = line.decode(errors="replace").rstrip("\r\n")
                if line.startswith("event: "):
                    event = line[7:]
                elif line.startswith("data: "):
                    data.append(line[6:])
                elif not line and event:
                    payload = json.loads("\n".join(data))
                    if event == "debug":
                        kind = payload.get("kind")
                        elapsed = parse_elapsed(payload.get("message", ""))
                        if kind == "first_model_delta" and elapsed is not None:
                            first_delta = elapsed
                        elif kind == "response_headers" and elapsed is not None:
                            response_headers = elapsed
                    elif event == "usage":
                        usage = payload
                    elif event == "chat_chunk":
                        content += payload.get("delta", "")
                    elif event == "tool":
                        tool_id = payload.get("id", "")
                        tool_name = payload.get("name", "")
                        calls.append(tool_name)
                        result = tool_result(
                            workspace, tool_name, payload.get("arguments", "{}")
                        )
                        post_json(
                            sidecar_base + "/v1/tool-result",
                            {
                                "request_id": request_id,
                                "tool_call_id": tool_id,
                                "result": result,
                            },
                        ).close()
                    elif event == "error":
                        error = payload.get("message", "sidecar error")
                    event, data = None, []
    except (OSError, urllib.error.URLError, json.JSONDecodeError) as exc:
        error = str(exc)
    total_ms = (time.perf_counter() - started) * 1000
    return {
        "name": name,
        "ok": error is None,
        "error": error,
        "first_delta_ms": first_delta,
        "response_headers_ms": response_headers,
        "total_ms": round(total_ms, 1),
        "calls": calls,
        "content_chars": len(content),
        "usage": usage or {},
    }


def make_messages(question: str, no_think_marker: bool = False) -> list[dict]:
    system = "You are a Markdown note assistant. Use the supplied tools when the user asks for an action."
    if no_think_marker:
        system = "/no_think\n" + system
    return [
        {
            "role": "system",
            "content": system,
        },
        {
            "role": "user",
            "content": f'The note currently open is titled "Benchmark Note".\n\nThe note current content is empty.\n\nUser request: {question}',
        },
    ]


def run_model(args, model: Path, template: Path | None) -> dict:
    sidecar = subprocess.Popen(
        [args.sidecar, "--port", str(args.sidecar_port)],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    server = None
    try:
        sidecar_base = f"http://127.0.0.1:{args.sidecar_port}"
        wait_health(sidecar_base)
        with tempfile.TemporaryDirectory(prefix="myelin-benchmark-") as tmp:
            slot_dir = Path(tmp) / "slots"
            slot_dir.mkdir()
            server_args = [
                args.llama,
                "-m",
                str(model),
                "--jinja",
                "--reasoning",
                "off",
                "--ctx-size",
                str(args.context_size),
                "--threads",
                str(args.threads),
                "--parallel",
                "1",
                "--no-warmup",
                "--slots",
                "--cache-reuse",
                "64",
                "--slot-save-path",
                str(slot_dir),
                "--port",
                str(args.llama_port),
            ]
            if template:
                server_args.extend(["--chat-template-file", str(template)])
            log_path = Path(tmp) / "llama.log"
            log_handle = log_path.open("w")
            server = subprocess.Popen(
                server_args,
                stdout=log_handle,
                stderr=log_handle,
            )
            wait_health(f"http://127.0.0.1:{args.llama_port}")
            llama_base = f"http://127.0.0.1:{args.llama_port}/v1"
            workspace = Path(tmp) / "workspace"
            workspace.mkdir()
            (workspace / "open.md").write_text("# Benchmark Note\n")
            (workspace / "other.md").write_text("Machine learning notes.\n")

            direct_options = {
                "strict": args.tool_strategy == "prompt",
                "prompt_tools": args.tool_strategy == "prompt",
                "max_calls": 2,
                "intent_is_tool": False,
                "friendly_results": True,
                "chat_mode": True,
                "no_think": args.sidecar_no_think,
                "no_think_prefill": False,
                "external": False,
            }
            tool_options = {
                "strict": args.tool_strategy == "prompt",
                "prompt_tools": args.tool_strategy == "prompt",
                # The host overrides deterministic tool turns to one call.
                "max_calls": 1,
                "intent_is_tool": True,
                "friendly_results": True,
                "chat_mode": False,
                "no_think": args.sidecar_no_think,
                "no_think_prefill": False,
                "external": False,
            }
            if args.template_kwargs:
                direct_options["template_kwargs"] = args.template_kwargs
                tool_options["template_kwargs"] = args.template_kwargs
            if args.tool_choice:
                tool_options["tool_choice"] = args.tool_choice
            results = []
            results.append(
                run_request(
                    sidecar_base,
                    llama_base,
                    str(model),
                    "chat-cold",
                    make_messages("What is this empty note about?", args.no_think_marker),
                    TOOLS,
                    direct_options,
                    workspace,
                    args.max_tokens,
                )
            )
            results.append(
                run_request(
                    sidecar_base,
                    llama_base,
                    str(model),
                    "chat-cache-followup",
                    make_messages("What is the title of the note?", args.no_think_marker),
                    TOOLS,
                    direct_options,
                    workspace,
                    args.max_tokens,
                )
            )
            for name, question, expected in [
                ("write-note", "Rewrite the note as a short Markdown note about Rust.", "write_note"),
                ("search-notes", "Search my other notes for machine learning.", "search_notes"),
                ("read-note", "Read the note with id other and summarize it.", "read_note"),
            ]:
                result = run_request(
                    sidecar_base,
                    llama_base,
                    str(model),
                    name,
                    make_messages(question, args.no_think_marker),
                    TOOLS if name != "write-note" else [TOOLS[0]],
                    {
                        **tool_options,
                        # This is the production focused Write contract. It
                        # forces the sidecar to use the one armed-target tool
                        # rather than broad operation-mode behavior.
                        "targeted_write": name == "write-note",
                        "selection_scoped": name == "write-note",
                    },
                    workspace,
                    args.max_tokens,
                )
                result["expected_tool"] = expected
                result["tool_ok"] = expected in result["calls"]
                results.append(result)
            return {"model": str(model), "template": str(template) if template else None, "results": results}
    finally:
        if server is not None:
            server.send_signal(signal.SIGTERM)
            try:
                server.wait(timeout=8)
            except subprocess.TimeoutExpired:
                server.kill()
        sidecar.send_signal(signal.SIGTERM)
        try:
            sidecar.wait(timeout=8)
        except subprocess.TimeoutExpired:
            sidecar.kill()


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--model", action="append", required=True)
    parser.add_argument("--template", action="append", default=[])
    parser.add_argument("--llama", required=True)
    parser.add_argument("--sidecar", required=True)
    parser.add_argument("--llama-port", type=int, default=18092)
    parser.add_argument("--sidecar-port", type=int, default=18091)
    parser.add_argument("--threads", type=int, default=4)
    parser.add_argument("--context-size", type=int, default=4096)
    parser.add_argument("--max-tokens", type=int, default=256)
    parser.add_argument("--tool-strategy", choices=("prompt", "native"), default="prompt")
    parser.add_argument("--no-think-marker", action="store_true")
    parser.add_argument("--sidecar-no-think", action="store_true")
    parser.add_argument("--template-kwargs", default=None)
    parser.add_argument("--tool-choice", default=None)
    parser.add_argument("--output", type=Path, default=None)
    args = parser.parse_args()
    if len(args.template) not in (0, len(args.model)):
        parser.error("pass either no --template values or one template per --model")
    templates = [Path(p) for p in args.template] if args.template else [None] * len(args.model)
    reports = []
    for model, template in zip(args.model, templates):
        print(f"Benchmarking {model}", flush=True)
        reports.append(run_model(args, Path(model), template))
    payload = {"generated_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()), "reports": reports}
    output = json.dumps(payload, indent=2)
    if args.output:
        args.output.write_text(output + "\n")
    print(output)
    failed = [
        result
        for report in reports
        for result in report["results"]
        if not result["ok"] or ("tool_ok" in result and not result["tool_ok"])
    ]
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())
