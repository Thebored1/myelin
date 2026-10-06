# Agent notes — Myelin

## Versioning

The app version is `0.x.x` and MUST NOT be updated without explicit user instruction.
Never bump it as part of unrelated work, and never "fix" a version drift on your own.

When the user explicitly instructs a version change, update ALL of the following to the
same value so the native and frontend sides never disagree:

- `package.json` — `version`
- `package-lock.json` — `version` at the root AND in the `packages[""]` entry
- `src-tauri/Cargo.toml` — `version`
- `src-tauri/Cargo.lock` — the `name = "myelin"` package entry's `version`
- `src-tauri/tauri.conf.json` — `version`

## Rust workspace

`src-tauri/Cargo.toml` is a Cargo **workspace root** covering all three crates: the app
(`myelin`), `edit-core`, and `openharn-myelin`. There is one `Cargo.lock` and one
`target/` at `src-tauri/`.

Consequences to remember:

- Run cargo from the workspace root, or pass `--manifest-path src-tauri/Cargo.toml -p <crate>`.
- `[profile.*]` is only honoured in the root manifest. The sidecar's size-first release
  settings live there as `[profile.sidecar]`; `src-tauri/openharn-myelin/install.sh`
  builds with `--profile sidecar` and copies from `src-tauri/target/sidecar/`.
- `cargo fmt --all` / `cargo check --workspace` cover every crate.

## License / repository metadata

The MIT SPDX identifier and the repository URL are mirrored in `package.json` and
`src-tauri/Cargo.toml`. Keep them in sync with the GitHub remote
(`https://github.com/Thebored1/myelin`).
