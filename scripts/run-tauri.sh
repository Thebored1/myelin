#!/usr/bin/env bash
set -euo pipefail

# WebKitGTK can load the page but produce a black, non-interactive surface when
# a Wayland session is forced through XWayland. Keep the native backend when the
# surrounding environment has supplied that mismatch for us.
if [[ "${XDG_SESSION_TYPE:-}" == "wayland" && "${GDK_BACKEND:-}" == "x11" ]]; then
	export GDK_BACKEND=wayland
fi

project_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
exec "${project_root}/node_modules/.bin/tauri" "$@"
