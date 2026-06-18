# llama.cpp compute backends

Myelin runs inference by launching `llama-server` (from llama.cpp) and talking
to it over HTTP. Generation speed depends almost entirely on whether the model
is offloaded to a **GPU** or stuck on the **CPU**. A CPU-only build of
llama.cpp silently ignores `--n-gpu-layers`, so it is the #1 cause of "the CLI
is 3× faster than the app."

## How backend selection works

On launch the app picks the best available binary for the machine, in this
order, and **falls back automatically** if one fails to start:

| OS | Preference order |
|----|------------------|
| Windows / Linux, NVIDIA present | `cuda` → `vulkan` → `cpu` |
| Windows / Linux, no NVIDIA | `vulkan` → `cpu` |
| macOS | `metal` → `cpu` |

NVIDIA is detected via `nvidia-smi`. Vulkan covers AMD/Intel/NVIDIA GPUs and
degrades to CPU on its own, so it is the safe universal fallback. The active
backend is shown in **Settings → Llama-Server**, and the app logs a warning if
a GPU was requested but the model ended up on the CPU.

## Folder layout

Put each backend's binaries in a subfolder named after the backend, next to
your `llama-server` binary (the folder configured in Settings), **or** under
`<app-data>/bin/`:

```
bin/
  cuda/    llama-server(.exe) + ggml-cuda.dll + cudart-*.dll   (NVIDIA, fastest)
  vulkan/  llama-server(.exe) + ggml-vulkan.dll                (any GPU)
  cpu/     llama-server(.exe) + ggml-cpu-*.dll                 (fallback)
  llama-server(.exe)                                           (legacy flat = CPU)
```

- `<app-data>` on Windows is
  `C:\Users\<you>\AppData\Roaming\com.paper.myelin`.
- You only need the backends relevant to your machine. A `cuda/` + `cpu/` pair
  is enough on an NVIDIA box.
- Each subfolder is a full extraction of the matching llama.cpp release zip —
  the `.dll`s must sit beside the `.exe`.

### Confirm a build actually uses the GPU

```sh
# from inside the backend folder, e.g. bin/cuda
./llama-bench.exe -m "C:/path/to/model.gguf" -ngl 999 -n 64
```

Look for `loaded CUDA backend` (or `Vulkan`/`Metal`) and a `backend | CUDA`
column. A CPU-only build prints only `loaded CPU backend`.

## Where to get GPU builds

Download from <https://github.com/ggml-org/llama.cpp/releases>:

- **Windows NVIDIA:** `llama-*-bin-win-cuda-x64.zip` **plus** the matching
  `cudart-llama-*.zip` (the CUDA runtime DLLs) — extract both into `bin/cuda/`.
- **Windows any GPU:** `llama-*-bin-win-vulkan-x64.zip` → `bin/vulkan/`.
- **Linux any GPU:** `llama-*-bin-ubuntu-vulkan-x64.tar.gz` → `bin/vulkan/`.
  (There is no prebuilt Linux CUDA tarball — Vulkan covers NVIDIA on Linux too.)
- **Linux CPU:** `llama-*-bin-ubuntu-x64.tar.gz` → `bin/cpu/`.
- **macOS:** the standard `llama-*-bin-macos-arm64.zip` already includes Metal →
  `bin/metal/`.

## In-app downloads

Settings → **Installed backends** lists the backends available for the current
OS and offers a **Download** button for any not yet installed (e.g. CUDA on an
NVIDIA Windows box). Downloads land in `<app-data>/bin/<backend>/` and are picked
up immediately. CPU + Vulkan are bundled with the app, so this is mainly for
adding CUDA. The release tag is pinned in `LLAMA_RELEASE_TAG` (llama_server.rs).

## Bundling for release

The installer ships CPU + Vulkan so users get GPU acceleration with zero setup.
Before `tauri build`, place the per-OS builds under `src-tauri/resources/bin/`:

```
src-tauri/resources/bin/
  cpu/      <- llama-server(.exe) + CPU ggml libs
  vulkan/   <- llama-server(.exe) + ggml-vulkan
```

`tauri.conf.json` maps `resources/bin` → `bin`, and at runtime the app adds the
resource `bin/` as a (lowest-priority) tiering root. So resolution order is:
user-downloaded (`<app-data>/bin`) → bundled (`<resources>/bin`). The folder is
gitignored (binaries are large) but its structure is kept.

## Building from source (cross-platform)

`src-tauri/.cargo/config.toml` is kept cross-platform: it only caps build
parallelism and applies `crt-static` on the Windows target. Everything else is
per-OS:

- **Linux:** no extra setup. tectonic uses system libraries (`pkg-config`).
  Needs the usual Tauri deps (`libwebkit2gtk-4.1`, GTK3, `libsoup-3.0`,
  `librsvg2`) plus a C/C++ toolchain. The harfbuzz symbol clash with the system
  text stack is handled in `build.rs` (`-Wl,--exclude-libs,ALL`).
- **Windows:** tectonic's C/C++ deps are built via **vcpkg**, so the build needs
  these environment variables (set them as **user** env vars, not in the repo, so
  Linux/macOS stay clean):

  ```
  TECTONIC_DEP_BACKEND = vcpkg
  VCPKGRS_TRIPLET      = x64-windows-static-release
  VCPKG_ROOT           = <repo>\src-tauri\target\vcpkg
  CXXFLAGS             = /std:c++17
  ```

  Run `cargo vcpkg build` once to populate `target/vcpkg`. Note: a change to
  `.cargo/config.toml` forces a full rebuild, and a clean rebuild of host
  proc-macros with `crt-static` can fail to link; if that happens, build with an
  explicit `--target x86_64-pc-windows-msvc` so the flag stays off host units.
- **macOS:** no extra setup; Metal is in the standard toolchain.

## Adaptive GPU offload

Settings → **Advanced AI Configuration → Adaptive GPU offload** (on by default)
makes the app size the launch to the machine automatically, so the same build
runs on a 512 MB iGPU and a 24 GB dGPU without manual tuning. The principle is
**consistent context, variable performance**.

How it works (`llama_server.rs`):

- **Keep the KV cache in RAM** (`--no-kv-offload`) so context size doesn't
  compete for VRAM — a large (32k) window fits on any GPU. `--flash-attn on` and
  a small `--ubatch-size` keep the GPU compute buffer bounded.
- **Take all the VRAM** (`--n-gpu-layers 999`); weights that don't fit spill to
  GTT/system RAM. On a dGPU this fills real VRAM (big win); on an iGPU it's
  mostly GTT (modest win) — both work, neither is hard-coded.
- **Clamp context to fit RAM**, not VRAM: the launcher reads the model's KV
  geometry from the GGUF header (a tiny built-in parser in `gguf.rs`) and the
  system's available RAM (`sysinfo`), then caps the 32k target so the KV cache
  fits ~60% of free RAM. This is what prevents the "huge prompt → out-of-memory →
  GPU device-lost" crash.
- **Retry instead of predict**: if a launch fails (OOM at load), it relaunches
  the same backend with a smaller context, then fewer GPU layers, before falling
  through to the next backend (ultimately CPU). A crash mid-reply is detected and
  the server is relaunched.

A cross-platform free-VRAM probe (AMD sysfs / `nvidia-smi` on Linux; DXGI/Metal
elsewhere — see `free_device_local_vram`) is logged and can later pick a smarter
starting `-ngl`, but the design intentionally doesn't depend on predicting exact
VRAM. Turn the toggle **off** to use the manual Context Size / GPU Layers fields
verbatim.

## Adaptive offload (auto)

Settings → **Advanced AI Configuration → Adaptive GPU offload** (default **on**)
sizes everything per machine + model so it uses the GPU for what it can without
running out of VRAM — *consistent context, variable performance*:

- **KV cache stays in system RAM** (`--no-kv-offload`), so context size doesn't
  compete for VRAM — a large context fits on any GPU, even a 512 MB iGPU.
- **Targets a 32k context**, clamped down only if RAM can't hold the KV cache
  (estimated from GGUF metadata: `layers × kv_heads × head_dim`, read by a small
  built-in GGUF parser).
- **Requests full offload** (`-ngl 999`) + **flash attention**; weights fill real
  VRAM and spill to GTT on shared-memory iGPUs.
- **Degrades on failure**: if a launch fails (e.g. device-lost / OOM) it retries
  with a smaller context, then fewer layers, then falls through to the next
  backend / CPU — instead of predicting exact VRAM up front.
- **Recovers from a mid-generation GPU crash** by relaunching.

VRAM is detected best-effort (AMD sysfs / `nvidia-smi` on Linux; DXGI/Metal
later) and used only as a hint — the retry loop is what guarantees a working
launch. Turn the toggle **off** to set Context Size / GPU Layers manually.

## Compute device: GPU vs Vulkan

The Settings compute selector has two choices — there is **no manual CPU or
per-device option**, because the app always manages CPU fallback and offload
itself:

- **GPU** (performance) — uses the fastest available GPU: CUDA on an NVIDIA
  discrete card, otherwise Vulkan. `backend_preference = "gpu"`.
- **Vulkan** (power-saving) — forces the Vulkan backend and, on a machine that
  also has a discrete GPU, **auto-pins the integrated GPU** (matched by name)
  via `--device`, so heavy work stays off the power-hungry dGPU.
  `backend_preference = "vulkan"`.

On a machine with **no discrete GPU**, the **GPU** option is disabled (with a
note) and the app uses **Vulkan** on the integrated GPU. The "Running on …"
badge polls the provider status, so it reflects the live backend as the server
starts / restarts / recovers.

Either choice still falls back through Vulkan → CPU automatically if the
preferred path is unavailable (adaptive offload + retry loop handle it).

## Tool calling

The launcher passes **`--jinja`** so llama-server uses each model's embedded
chat template to render tool definitions. This is required for correct tool
calling on models like **LFM2** (which, unlike Qwen, has no built-in C++
template path) — without it the model mis-selects or appears to "lose" its
tools. Note context is also passed as labelled reference (separate from the
user's request) so models don't echo the note's current content as their answer.

## Thinking / reasoning toggle

Settings → **Advanced AI Configuration → Model thinking / reasoning** toggles
whether the model reasons before answering. It's model-agnostic: the launcher
passes `--reasoning on|off` to llama-server, which drives each model's chat
template (Qwen, LFM, …) rather than a model-specific prompt token. Off (default)
is faster and emits no hidden reasoning tokens; on may improve accuracy at the
cost of speed. Changing it relaunches the server.

## Override / power users

- `MYELIN_LLAMA_SERVER_PATH` env var hard-pins a single executable and skips
  tiering entirely.
- The Settings compute selector now resolves the binary automatically; there is
  no manual executable picker.

## Mobile (Android / iOS) — not supported by this model

The launch-a-server approach does not work on iOS (the sandbox forbids spawning
bundled executables) and is fragile on Android. Mobile support requires linking
llama.cpp **in-process** (Metal on iOS, Vulkan/CPU on Android) — a separate
effort from the desktop backend tiering described here.
