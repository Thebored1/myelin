# Myelin

Cross-platform local-first AI notes app built with Tauri 2, SvelteKit, and a Rust-first core. Myelin is built for students, researchers, and developers, seamlessly supporting multiple document types with zero external dependencies.

## Key Features

- **Markdown-first**: Standard `.md` notes are the primary source of truth, parsed and indexed locally.
- **First-Class LaTeX (`.tex`)**: Edit LaTeX documents directly in the app. Myelin embeds the Tectonic LaTeX engine (written in Rust) to compile documents entirely in-memory and render the PDF side-by-side—no `pdflatex` or massive LaTeX distribution required on the host system.
- **First-Class Jupyter Notebooks (`.ipynb`)**: Open and run Jupyter notebooks directly in the app. Python execution is powered by Pyodide (WebAssembly), which runs completely inside the browser/webview environment without requiring Python to be installed on the host OS.
- **Split-Pane Viewer**: View source material (PDFs, Web pages, etc.) side-by-side with your working documents.
- **Local AI & Vector Search**: Uses LanceDB for local vector indexing to provide intelligent search over your notes.

## Sidecar tool self-test

The sidecar can be tested independently of the desktop UI with a temporary Markdown workspace. This exercises the real sidecar SSE protocol, including tool events and `/v1/tool-result`, and prints every tool call so missing calls are visible:

```bash
# Test every GGUF in ~/Downloads
npm run test:sidecar -- --models-dir ~/Downloads

# Or test selected models
npm run test:sidecar -- ~/Downloads/model-a.gguf ~/Downloads/model-b.gguf
```

The test never changes user notes. `write_note`, `read_note`, and `search_notes` operate on a temporary `open.md`/`other.md`; network-facing tools receive a synthetic result. Override `LLAMA_SERVER_BIN` and `SIDECAR_BIN` when the binaries are not on `PATH`. A non-zero exit status means at least one expected tool call was not observed.

## Setup and Development

Myelin relies heavily on native Rust libraries (like Tectonic) to achieve a zero-dependency runtime.

### Prerequisites (All Platforms)

1. **Node.js** (v21.7.3+) and **npm** (v10.5.0+). The current dependency lockfile requires these minimum versions.
2. **Rust** (stable toolchain) and Cargo.
3. **Tauri native build prerequisites** (C++ Build Tools on Windows, Xcode tools on macOS, WebKitGTK development headers on Linux).
4. **Protocol Buffers compiler (`protoc`)**. It is required by LanceDB's `lance-encoding` dependency during the Rust build.

### Platform-Specific Backend Setup

Because the Tectonic LaTeX engine requires several native C/C++ libraries (ICU, HarfBuzz, Fontconfig, FreeType, OpenSSL, libpng, zlib), the compilation process differs by OS to ensure the final application remains self-contained.

#### Windows

To create a fully self-contained `.exe` or `.msi` without requiring users to install dynamic libraries, Myelin relies on `vcpkg` for static linking on Windows.

1. Ensure the `vcpkg` cargo backend is configured. Inside `src-tauri/.cargo/config.toml`, ensure the following environment variables are set:
   ```toml
   [env]
   TECTONIC_DEP_BACKEND = "vcpkg"
   VCPKGRS_TRIPLET = "x64-windows-static-release"
   VCPKG_ROOT = "C:\\path\\to\\myelin\\src-tauri\\target\\vcpkg"
   CXXFLAGS = "/std:c++17"
   ```
2. Build the C++ dependencies statically via `cargo-vcpkg`:
   ```bash
   cd src-tauri
   cargo install cargo-vcpkg
   cargo vcpkg build
   ```
   _Note: If some dependencies are not compiled correctly for the static release triplet, you may need to install them manually using the bootstrapped vcpkg executable:_
   ```bash
   target\vcpkg\vcpkg install icu:x64-windows-static-release harfbuzz[graphite2]:x64-windows-static-release freetype:x64-windows-static-release fontconfig:x64-windows-static-release libpng:x64-windows-static-release zlib:x64-windows-static-release openssl:x64-windows-static-release
   ```

#### macOS

macOS comes with many required libraries, but you will need `pkg-config` and `icu4c`.

```bash
brew install pkg-config icu4c openssl fontconfig harfbuzz freetype
```

#### Linux (Ubuntu/Debian)

Install Node.js, Rust/Cargo, the Tauri WebKitGTK headers, the Tectonic native libraries, a C/C++ toolchain, and Protocol Buffers:

```bash
sudo apt-get update
sudo apt-get install -y \
  nodejs npm cargo rustc build-essential pkg-config protobuf-compiler \
  libwebkit2gtk-4.1-dev \
  libicu-dev libharfbuzz-dev libfontconfig1-dev libfreetype6-dev \
  libssl-dev zlib1g-dev libpng-dev
```

Ubuntu's packaged npm version may be older than the version required by the lockfile. Check it with `npm --version`, then upgrade if it is below 10.5.0:

```bash
sudo npm install -g npm@11
```

Verify the toolchain before building:

```bash
node --version
npm --version
cargo --version
rustc --version
protoc --version
```

## Running the App

Once the dependencies are configured, you can start the development server:

```bash
npm install
npm run build:sidecar
npm run tauri dev
```

### Required Openharn sidecar

Myelin's AI agent and tool-calling features use the `openharn-myelin` Rust
sidecar. The main window can open without it, but AI requests that use the
agent/tool layer will fail unless the sidecar binary is available.

Build and install the sidecar into the location Myelin checks automatically:

```bash
npm run build:sidecar
```

This places the binary at `src-tauri/resources/bin/openharn-myelin`. Run the
command before `npm run tauri dev`, and run it again before `npm run tauri build`
so the binary is included in a packaged release. Alternatively, open Settings
→ Agent (openharn) and browse to an existing binary, or set
`OPENHARN_MYELIN_BIN` to its path.

To build for production:

```bash
npm run build:sidecar
npm run tauri build
```

For troubleshooting and platform-specific details, see
[`docs/openharn-sidecar.md`](docs/openharn-sidecar.md).

## Architecture Overview

- `src-tauri/`: Contains the portable app core and embedded LaTeX engine (Tectonic). Rust owns note CRUD, indexing, search, and workspace state.
- `src/`: The SvelteKit frontend containing the thin desktop UI shell, Pyodide WASM integration, and visual editors.
- Notes live in a user-selected workspace directory.
- App-managed indexes/settings live in the platform-specific app data directory.
