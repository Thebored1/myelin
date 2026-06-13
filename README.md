# Myelin

Cross-platform local-first AI notes app built with Tauri 2, SvelteKit, and a Rust-first core. Myelin is built for students, researchers, and developers, seamlessly supporting multiple document types with zero external dependencies.

## Key Features

- **Markdown-first**: Standard `.md` notes are the primary source of truth, parsed and indexed locally.
- **First-Class LaTeX (`.tex`)**: Edit LaTeX documents directly in the app. Myelin embeds the Tectonic LaTeX engine (written in Rust) to compile documents entirely in-memory and render the PDF side-by-side—no `pdflatex` or massive LaTeX distribution required on the host system.
- **First-Class Jupyter Notebooks (`.ipynb`)**: Open and run Jupyter notebooks directly in the app. Python execution is powered by Pyodide (WebAssembly), which runs completely inside the browser/webview environment without requiring Python to be installed on the host OS.
- **Split-Pane Viewer**: View source material (PDFs, Web pages, etc.) side-by-side with your working documents.
- **Local AI & Vector Search**: Uses LanceDB for local vector indexing to provide intelligent search over your notes.

## Setup and Development

Myelin relies heavily on native Rust libraries (like Tectonic) to achieve a zero-dependency runtime.

### Prerequisites (All Platforms)
1. **Node.js** (v18+) and **npm**
2. **Rust** (stable toolchain) via rustup
3. **Tauri CLI** prerequisites (C++ Build Tools on Windows, Xcode tools on macOS, webkit2gtk on Linux)

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
   *Note: If some dependencies are not compiled correctly for the static release triplet, you may need to install them manually using the bootstrapped vcpkg executable:*
   ```bash
   target\vcpkg\vcpkg install icu:x64-windows-static-release harfbuzz[graphite2]:x64-windows-static-release freetype:x64-windows-static-release fontconfig:x64-windows-static-release libpng:x64-windows-static-release zlib:x64-windows-static-release openssl:x64-windows-static-release
   ```

#### macOS
macOS comes with many required libraries, but you will need `pkg-config` and `icu4c`.
```bash
brew install pkg-config icu4c openssl fontconfig harfbuzz freetype
```

#### Linux (Ubuntu/Debian)
Install the standard `pkg-config` and development headers.
```bash
sudo apt-get install pkg-config libicu-dev libharfbuzz-dev libfontconfig1-dev libfreetype6-dev libssl-dev zlib1g-dev libpng-dev
```

## Running the App

Once the dependencies are configured, you can start the development server:

```bash
npm install
npm run tauri dev
```

To build for production:

```bash
npm run tauri build
```

## Architecture Overview

- `src-tauri/`: Contains the portable app core and embedded LaTeX engine (Tectonic). Rust owns note CRUD, indexing, search, and workspace state.
- `src/`: The SvelteKit frontend containing the thin desktop UI shell, Pyodide WASM integration, and visual editors.
- Notes live in a user-selected workspace directory.
- App-managed indexes/settings live in the platform-specific app data directory.
