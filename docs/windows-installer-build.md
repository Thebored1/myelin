# Building the Windows installer

Produces a single self-contained NSIS installer:
`src-tauri/target/release/bundle/nsis/myelin_0.2.0_x64-setup.exe` (~56 MB).
It bundles the app **and** the CPU `llama-server` runtime, so inference works
offline out of the box. The model (a `.gguf`) is **not** bundled — users pick one
in Settings.

A Windows installer must be built **on Windows** (NSIS + MSVC); it can't be
cross-compiled from Linux.

## One-time setup: vcpkg static C libs (for `tectonic`)

`tectonic` (LaTeX → PDF for `.tex` notes) links several static C libraries that a
release build compiles from source via vcpkg. Dev builds skip this (cached), but a
release build needs them present.

1. Clone + bootstrap vcpkg at `src-tauri/target/vcpkg` (matches the `VCPKG_ROOT`
   user env var). `Cargo.toml` has the `cargo-vcpkg` metadata (`branch = "master"`),
   or do it manually:
   ```sh
   git clone --depth 1 https://github.com/microsoft/vcpkg src-tauri/target/vcpkg
   src-tauri/target/vcpkg/bootstrap-vcpkg.bat -disableMetrics
   ```
2. Install the deps for the **`x64-windows-static`** triplet (ICU is the long pole,
   ~30–60 min; `fontconfig` is required on Windows too — `tectonic_bridge_fontconfig`
   probes for it):
   ```sh
   src-tauri/target/vcpkg/vcpkg.exe install \
     libpng freetype harfbuzz[graphite2] icu zlib openssl fontconfig \
     --triplet x64-windows-static --clean-after-build
   ```

> The `VCPKGRS_TRIPLET` user env var may be set to a stale custom
> `x64-windows-static-release`. Build with `VCPKGRS_TRIPLET=x64-windows-static`
> (matching what's installed above) or `tectonic`'s build script won't find the libs.

## Stage the inference runtime

The installer bundles `src-tauri/resources/bin` → `bin` (git-ignored, so staged
locally per build). Copy the CPU backend in:

```powershell
$src="$env:APPDATA\com.paper.myelin\bin\cpu"; $dst="src-tauri\resources\bin\cpu"
New-Item -ItemType Directory -Force $dst | Out-Null
Copy-Item "$src\llama-server.exe" $dst -Force
Copy-Item "$src\*.dll" $dst -Force   # ~30 files, ~46 MB
```

## Build

`bundle.targets` is `["nsis"]` in `tauri.conf.json` (one installer, no separate
`.msi`). Then:

```powershell
$env:VCPKGRS_TRIPLET="x64-windows-static"
npm run tauri build
```

Output: `src-tauri/target/release/bundle/nsis/myelin_0.2.0_x64-setup.exe`.

After the one-time vcpkg setup, rebuilds are incremental (a few minutes).
