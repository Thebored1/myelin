#!/usr/bin/env bash
# Build the openharn-myelin sidecar and install it into the bundled bin dir so
# Myelin can find it at runtime (resources/bin/openharn-myelin, see
# src-tauri/src/sidecar.rs::resolve_sidecar_bin). Run this before `tauri dev` /
# `tauri build` (or via `npm run build:sidecar`).
set -euo pipefail

cd "$(dirname "$0")"

PROFILE="${1:-debug}"   # pass "release" for a release build
case "$PROFILE" in
  release)
    CARGO_PROFILE=release
    TARGET_DIR=release
    ;;
  debug|dev)
    CARGO_PROFILE=dev
    TARGET_DIR=debug
    ;;
  *)
    echo "[install] unknown profile '$PROFILE' (expected debug/dev or release)" >&2
    exit 2
    ;;
esac

echo "[install] building openharn-myelin ($CARGO_PROFILE)"
cargo build ${CARGO_PROFILE:+--profile "$CARGO_PROFILE"}

SRC="target/$TARGET_DIR/openharn-myelin"
if [ ! -f "$SRC" ]; then
  echo "[install] expected binary at $SRC not found" >&2
  exit 1
fi

DEST_DIR="../resources/bin"
mkdir -p "$DEST_DIR"
DEST="$DEST_DIR/openharn-myelin"
if [ "$(uname -s)" = "Windows_NT" ] || [ "${OSTYPE:-}" = "msys" ]; then
  DEST="$DEST.exe"
fi

cp "$SRC" "$DEST"
echo "[install] installed sidecar to $DEST"
