# Openharn Sidecar

Myelin uses the `openharn-myelin` Rust sidecar for AI agent behavior and tool
calling. It owns the agent loop while Myelin performs the real note, search,
document, and web tool operations.

The desktop app can open without the sidecar, but agent-backed AI requests will
not work until the binary is built, bundled, or selected manually.

## Development setup

From the repository root, build and install the sidecar before starting Myelin:

```bash
npm install
npm run build:sidecar
npm run tauri dev
```

`npm run build:sidecar` compiles `src-tauri/openharn-myelin` in release mode
and copies the executable to:

```text
src-tauri/resources/bin/openharn-myelin
```

Run the command again after changing the sidecar source.

## Packaging a release

Build the sidecar before running the Tauri packaging command:

```bash
npm run build:sidecar
npm run tauri build
```

The Tauri configuration maps `src-tauri/resources/bin` into the packaged
application's resource `bin` directory. If the sidecar was not built first,
the package can be created successfully but AI agent/tool-calling requests will
report that the binary is missing.

## Using an existing binary

Instead of building locally, open Settings → Agent (openharn) and use Browse to
select an existing `openharn-myelin` executable. For development and testing,
you can also set:

```bash
export OPENHARN_MYELIN_BIN=/absolute/path/to/openharn-myelin
```

The explicit Settings path takes precedence over the environment variable;
otherwise Myelin searches its bundled/resource directories automatically.

## Troubleshooting

If Myelin reports `openharn-myelin sidecar binary not found`:

1. Run `npm run build:sidecar` from the repository root.
2. Confirm that `src-tauri/resources/bin/openharn-myelin` exists.
3. Restart `npm run tauri dev` so the resource directory is reloaded.
4. Or select the executable manually in Settings → Agent (openharn).

The sidecar is separate from `llama-server`: both binaries are needed for the
agent/tool-calling path.
