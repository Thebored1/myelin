#!/usr/bin/env bash
# Build the openharn-myelin sidecar and install it into the bundled bin dir so
# Myelin can find it at runtime (resources/bin/openharn-myelin, see
# src-tauri/src/sidecar.rs::resolve_sidecar_bin). Run this before `tauri dev` /
# `tauri build` (or via `npm run build:sidecar`).
set -euo pipefail

cd "$(dirname "$0")"

# This crate is a workspace member, so cargo resolves the profile from the
# workspace root manifest (src-tauri/Cargo.toml). The size-first release
# settings are the `sidecar` profile defined there.
PROFILE="${1:-debug}"   # pass "release" for a size-optimized build
case "$PROFILE" in
  release)
    CARGO_PROFILE=sidecar
    TARGET_DIR=sidecar
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
cargo build --manifest-path ../Cargo.toml -p openharn-myelin --profile "$CARGO_PROFILE"

# The workspace shares one target directory at the workspace root.
SRC="../target/$TARGET_DIR/openharn-myelin"
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
