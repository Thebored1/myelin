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

## License / repository metadata

The MIT SPDX identifier and the repository URL are mirrored in `package.json` and
`src-tauri/Cargo.toml`. Keep them in sync with the GitHub remote
(`https://github.com/Thebored1/myelin`).
