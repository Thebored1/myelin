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
- **Linux NVIDIA:** `llama-*-bin-ubuntu-cuda-x64.zip` → `bin/cuda/`.
- **macOS:** the standard `llama-*-bin-macos-arm64.zip` already includes Metal →
  `bin/metal/`.

## Override / power users

- `MYELIN_LLAMA_SERVER_PATH` env var hard-pins a single executable and skips
  tiering entirely.
- The **Browse…** button in Settings sets `executablePath`; its parent folder
  becomes a tiering root, so dropping a `cuda/` folder beside it is enough.

## Mobile (Android / iOS) — not supported by this model

The launch-a-server approach does not work on iOS (the sandbox forbids spawning
bundled executables) and is fragile on Android. Mobile support requires linking
llama.cpp **in-process** (Metal on iOS, Vulkan/CPU on Android) — a separate
effort from the desktop backend tiering described here.
