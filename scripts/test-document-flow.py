#!/usr/bin/env python3
"""Black-box document-chat/cache probe for a running Myelin model.

This uses a real PDF from the Myelin workspace, then exercises the same
prompt-cache sequence as the app: warm a pinned llama slot and issue a direct
document question. It can also send the resulting prompt through the running
Openharn sidecar, so cache usage and routing are observable without clicking
through the UI.

Examples:
  python3 scripts/test-document-flow.py
  python3 scripts/test-document-flow.py --sidecar-url http://127.0.0.1:8091
"""

from __future__ import annotations

import argparse
import concurrent.futures
import json
import subprocess
import time
import urllib.request
from pathlib import Path


DEFAULT_PDF = Path(
    "/home/paper/Documents/Workspace 01/"
    "Inference-Time Recovery of Tool-Use Capability in Low-Bit Quantised Large Language Models.pdf"
)
SYSTEM = (
    "You are Myelin's built-in AI assistant, powered by a local model. "
    "Answer the user's question directly from the supplied document evidence. "
    "Do not emit tool calls. Be concise unless detail is requested."
)


def post_json(url: str, body: dict, timeout: float = 180.0) -> dict:
    request = urllib.request.Request(
        url,
        data=json.dumps(body).encode(),
        headers={"Content-Type": "application/json"},
    )
    with urllib.request.urlopen(request, timeout=timeout) as response:
        return json.load(response)


def document_excerpt(pdf: Path, limit: int) -> str:
    text = subprocess.check_output(
        ["pdftotext", "-f", "1", "-l", "3", str(pdf), "-"],
        text=True,
    )
    return text[:limit]


def usage_report(value: dict, elapsed: float) -> dict:
    usage = value.get("usage", {})
    details = usage.get("prompt_tokens_details", {})
    timings = value.get("timings", {})
    prompt_tokens = usage.get("prompt_tokens") or timings.get("prompt_n") or 0
    cached_tokens = details.get("cached_tokens") or timings.get("cache_n") or 0
    return {
        "elapsed_s": round(elapsed, 2),
        "prompt_tokens": prompt_tokens,
        "cached_tokens": cached_tokens,
        "evaluated_tokens": prompt_tokens - cached_tokens,
        "prompt_ms": timings.get("prompt_ms"),
    }


def direct_request(messages: list[dict], model: str, slot: int = 0) -> dict:
    return {
        "model": model,
        "messages": messages,
        "temperature": 0.0,
        "max_tokens": 1,
        "stream": False,
        "cache_prompt": True,
        "id_slot": slot,
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--pdf", type=Path, default=DEFAULT_PDF)
    parser.add_argument("--llama-url", default="http://127.0.0.1:39281")
    parser.add_argument("--sidecar-url", default=None)
    parser.add_argument("--model", default="LFM2-8B-A1B-UD-Q4_K_XL.gguf")
    parser.add_argument("--question", default="Does this paper mention MiniCPM-V?")
    parser.add_argument("--evidence-chars", type=int, default=1800)
    parser.add_argument(
        "--simulate-retrieval-seconds",
        type=float,
        default=0.0,
        help="Overlap this delay with warm-up, matching AppState retrieval timing.",
    )
    args = parser.parse_args()

    if not args.pdf.exists():
        parser.error(f"PDF does not exist: {args.pdf}")

    evidence = document_excerpt(args.pdf, args.evidence_chars)
    system = f"{SYSTEM}\n\nDOCUMENT EVIDENCE:\n{evidence}"
    warmup = [{"role": "system", "content": system}, {"role": "user", "content": " "}]
    actual = [{"role": "system", "content": system}, {"role": "user", "content": args.question}]
    base = args.llama_url.rstrip("/")

    print(f"PDF: {args.pdf}")
    print(f"Evidence chars: {len(evidence)}")
    print("Warm-up: pinned id_slot=0")
    started = time.monotonic()
    if args.simulate_retrieval_seconds > 0:
        with concurrent.futures.ThreadPoolExecutor(max_workers=1) as pool:
            future = pool.submit(
                post_json,
                f"{base}/v1/chat/completions",
                direct_request(warmup, args.model),
            )
            time.sleep(args.simulate_retrieval_seconds)
            warm = future.result()
        print(f"Overlapped retrieval delay: {args.simulate_retrieval_seconds:.1f}s")
    else:
        warm = post_json(f"{base}/v1/chat/completions", direct_request(warmup, args.model))
    print(json.dumps(usage_report(warm, time.monotonic() - started), indent=2))

    print("Direct document request: pinned id_slot=0")
    started = time.monotonic()
    answer = post_json(f"{base}/v1/chat/completions", direct_request(actual, args.model))
    direct_usage = usage_report(answer, time.monotonic() - started)
    print(json.dumps(direct_usage, indent=2))

    if args.sidecar_url:
        request_id = f"document-flow-{time.time_ns()}"
        body = {
            "request_id": request_id,
            "base_url": base,
            "model": args.model,
            "temperature": 0.0,
            "max_tokens": 1,
            "messages": actual,
            "tools": [],
            "options": {
                "strict": True,
                "prompt_tools": True,
                "friendly_results": True,
                "intent_is_tool": False,
                "chat_mode": True,
                "no_think": True,
            },
            "session": {"slot_id": 0, "epoch": 1, "pass_kind": "direct answer"},
        }
        print("Sidecar route:")
        started = time.monotonic()
        request = urllib.request.Request(
            f"{args.sidecar_url.rstrip('/')}/v1/chat/stream",
            data=json.dumps(body).encode(),
            headers={"Content-Type": "application/json"},
        )
        usage = None
        with urllib.request.urlopen(request, timeout=180) as response:
            for raw_line in response:
                line = raw_line.decode(errors="replace").strip()
                if not line.startswith("data: "):
                    continue
                payload = json.loads(line[6:])
                if usage is None and "cached_tokens" in payload:
                    usage = payload
        print(json.dumps({"elapsed_s": round(time.monotonic() - started, 2), "usage": usage}, indent=2))

    if direct_usage.get("cached_tokens", 0) > 0:
        print("PASS: the document prompt reused cached KV.")
    elif direct_usage.get("evaluated_tokens", 0) <= 700:
        print("PASS: the compact document prompt stayed below 700 evaluated tokens.")
    else:
        print("FAIL: neither prompt KV reuse nor compact evaluation was achieved.")
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
