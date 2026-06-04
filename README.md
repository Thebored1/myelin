# myelin

Cross-platform local-first notes app built with Tauri 2, SvelteKit, and a Rust-first core.

## What is in place

- markdown files are the source of truth
- Rust owns note CRUD, indexing, search, and workspace state
- LanceDB is initialized as the local vector index backend
- the frontend only talks through Tauri commands and events
- filesystem handling is written with cross-platform path rules in mind

## Run

```bash
npm install
npm run tauri dev
```

## Current architecture

- `src-tauri/` contains the portable app core
- `src/routes/+page.svelte` is the thin desktop UI shell
- notes live in a user-selected workspace
- app-managed index/settings live in the platform app data directory

## Current AI status

The provider surface is already provider-agnostic, but the actual embedding runtime currently uses a portable local hashed fallback so the app works before wiring in Ollama or another real model backend.
