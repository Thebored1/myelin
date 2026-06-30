#!/usr/bin/env bash
# Probe what a GGUF model actually emits for a note-editing request, straight
# against llama-server (no GUI). This is how we caught the model labelling a
# whole-note rewrite as mode:"edit" with no `find`. Pairs with the Rust unit
# tests (`cargo test -p myelin --lib`) which cover the decision logic.
#
# Usage:
#   scripts/probe-toolcall.sh <model.gguf> [prompt] [llama-server-bin] [port]
#
# Env overrides: LLAMA_BIN, PORT.
set -u

MODEL="${1:?path to a .gguf model required}"
PROMPT="${2:-The note currently says: \"Cars are fast. They have engines. People drive them daily.\" Add markdown headings (## ) to organize it. Use write_note.}"
BIN="${3:-${LLAMA_BIN:-$HOME/.local/share/com.paper.myelin/bin/cpu/llama-server}}"
PORT="${4:-${PORT:-8099}}"

[ -x "$BIN" ] || { echo "llama-server not found/executable at: $BIN"; exit 1; }
[ -f "$MODEL" ] || { echo "model not found: $MODEL"; exit 1; }

pkill -f "llama-server.*--port $PORT" 2>/dev/null || true
sleep 1
"$BIN" -m "$MODEL" --jinja --ctx-size 4096 --port "$PORT" --no-warmup >/tmp/probe-llama.log 2>&1 &
SRV=$!
trap 'kill $SRV 2>/dev/null || true' EXIT

for _ in $(seq 1 90); do
  curl -s "http://127.0.0.1:$PORT/health" 2>/dev/null | grep -q '"status":"ok"' && break
  sleep 1
done
curl -s "http://127.0.0.1:$PORT/health" 2>/dev/null | grep -q '"status":"ok"' \
  || { echo "SERVER FAILED TO START"; tail -20 /tmp/probe-llama.log; exit 1; }

echo "MODEL: $(basename "$MODEL")"
python3 - "$PROMPT" > /tmp/probe-req.json <<'PY'
import json, sys
print(json.dumps({
  "model": "local", "stream": True, "temperature": 0.2,
  "messages": [
    {"role": "system", "content": "You are a note assistant. To change the open note, call write_note. mode replace sets the whole body to content."},
    {"role": "user", "content": sys.argv[1]},
  ],
  "tools": [{"type": "function", "function": {
    "name": "write_note",
    "description": "Write the open note. mode replace sets the whole body to content; append adds to the end; edit replaces the exact find snippet.",
    "parameters": {"type": "object", "properties": {
      "content": {"type": "string"}, "mode": {"type": "string", "enum": ["replace", "append", "edit"]}, "find": {"type": "string"}
    }, "required": ["content"]}}}],
}))
PY

curl -sN "http://127.0.0.1:$PORT/v1/chat/completions" \
  -H 'Content-Type: application/json' -d @/tmp/probe-req.json > /tmp/probe-resp.txt 2>&1

echo "tool_calls deltas: $(grep -c 'tool_calls' /tmp/probe-resp.txt)   content deltas: $(grep -c '"content":"' /tmp/probe-resp.txt)"
echo "--- assembled tool-call arguments ---"
# Concatenate every streamed arguments fragment to reconstruct the JSON the model sent.
grep -o '"arguments":"\([^"]\|\\"\)*"' /tmp/probe-resp.txt \
  | sed -E 's/^"arguments":"//; s/"$//' | tr -d '\n'
echo ""
echo "(full raw stream in /tmp/probe-resp.txt)"
