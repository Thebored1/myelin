#!/usr/bin/env bash
set -euo pipefail
: "${LLAMA_SERVER:?set LLAMA_SERVER to llama-server}"
: "${MODEL:?set MODEL to a GGUF path}"
HOST=${HOST:-127.0.0.1}; PORT=${PORT:-39282}; CTX=${CTX:-4096}
REQUEST=${REQUEST:-"Answer in one sentence: what is the purpose of this benchmark?"}
cleanup() { [[ -n "${SERVER_PID:-}" ]] && kill "$SERVER_PID" 2>/dev/null || true; }
trap cleanup EXIT
for threads in auto 6; do for ubatch in 256 512 1024; do for flash in on off; do for kv in f16 q8_0; do
  args=(--host "$HOST" --port "$PORT" --model "$MODEL" --ctx-size "$CTX" --n-gpu-layers 0 --parallel 1 --ubatch-size "$ubatch")
  [[ "$threads" != auto ]] && args+=(--threads "$threads")
  [[ "$flash" == on ]] && args+=(--flash-attn on)
  [[ "$kv" != f16 ]] && args+=(--cache-type-k "$kv" --cache-type-v "$kv")
  "$LLAMA_SERVER" "${args[@]}" >/dev/null 2>"${TMPDIR:-/tmp}/myelin-llama-bench.log" & SERVER_PID=$!
  for _ in {1..60}; do curl -sf "http://$HOST:$PORT/health" >/dev/null && break; sleep .25; done
  payload=$(printf '%s' "$REQUEST" | jq -Rs '{model:"bench",messages:[{role:"user",content:.}],stream:true,max_tokens:16}')
  first=$(curl -sN --max-time 90 -H 'content-type: application/json' -d "$payload" "http://$HOST:$PORT/v1/chat/completions" | awk '/^data: / {print; exit}') || first=error
  printf 'threads=%s ubatch=%s flash=%s kv=%s first_delta=%s\n' "$threads" "$ubatch" "$flash" "$kv" "$first"
  kill "$SERVER_PID" 2>/dev/null || true; wait "$SERVER_PID" 2>/dev/null || true; unset SERVER_PID; PORT=$((PORT + 1))
done; done; done; done
