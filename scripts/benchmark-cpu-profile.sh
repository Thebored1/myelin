#!/usr/bin/env bash
set -euo pipefail
: "${LLAMA_SERVER:?set LLAMA_SERVER to llama-server}"
: "${MODEL:?set MODEL to a GGUF path}"
HOST=${HOST:-127.0.0.1}; PORT=${PORT:-39282}; CTX=${CTX:-4096}
REQUEST=${REQUEST:-"Read the following benchmark context and summarize it in one sentence. "}
PHYSICAL_THREADS=${PHYSICAL_THREADS:-$(
  if command -v lscpu >/dev/null 2>&1; then
    lscpu -p=CORE,SOCKET | awk -F, '!/^#/ {seen[$1 FS $2]=1} END {print length(seen)}'
  elif command -v sysctl >/dev/null 2>&1; then
    sysctl -n hw.physicalcpu
  else
    getconf _NPROCESSORS_ONLN
  fi
)}
# A tiny prompt cannot expose prompt-batch throughput. Keep the default payload
# large enough to cross every swept ubatch boundary.
if [[ -z "${BENCH_PROMPT:-}" ]]; then
  printf -v BENCH_PROMPT '%*s' 160 ''
  BENCH_PROMPT=${BENCH_PROMPT// /"$REQUEST"}
fi
cleanup() { [[ -n "${SERVER_PID:-}" ]] && kill "$SERVER_PID" 2>/dev/null || true; }
trap cleanup EXIT
for threads in auto "$PHYSICAL_THREADS"; do for ubatch in 256 512 1024; do for flash in on off; do for kv in f16 q8_0; do
  args=(--host "$HOST" --port "$PORT" --model "$MODEL" --ctx-size "$CTX" --n-gpu-layers 0 --parallel 1 --ubatch-size "$ubatch")
  [[ "$threads" != auto ]] && args+=(--threads "$threads")
  [[ "$flash" == on ]] && args+=(--flash-attn on)
  [[ "$kv" != f16 ]] && args+=(--cache-type-k "$kv" --cache-type-v "$kv")
  "$LLAMA_SERVER" "${args[@]}" >/dev/null 2>"${TMPDIR:-/tmp}/myelin-llama-bench.log" & SERVER_PID=$!
  for _ in {1..60}; do curl -sf "http://$HOST:$PORT/health" >/dev/null && break; sleep .25; done
  payload=$(printf '%s' "$BENCH_PROMPT" | jq -Rs '{model:"bench",messages:[{role:"user",content:.}],stream:false,max_tokens:16,cache_prompt:false}')
  result=$(curl -sf --max-time 180 -H 'content-type: application/json' -d "$payload" "http://$HOST:$PORT/v1/chat/completions") || result='{}'
  prompt_tps=$(jq -r '.timings.prompt_per_second // "error"' <<<"$result")
  generation_tps=$(jq -r '.timings.predicted_per_second // "error"' <<<"$result")
  prompt_tokens=$(jq -r '.timings.prompt_n // .usage.prompt_tokens // "error"' <<<"$result")
  printf 'threads=%s ubatch=%s flash=%s kv=%s prompt_tokens=%s prompt_tps=%s generation_tps=%s\n' \
    "$threads" "$ubatch" "$flash" "$kv" "$prompt_tokens" "$prompt_tps" "$generation_tps"
  kill "$SERVER_PID" 2>/dev/null || true; wait "$SERVER_PID" 2>/dev/null || true; unset SERVER_PID; PORT=$((PORT + 1))
done; done; done; done
