# AI runtime configuration

Myelin keeps technical AI settings in the application data file
`ai-config.json`. It supports named model profiles and local
llama-server-compatible runtimes. The schema is written next to it as
`ai-config.schema.json`, so editors can provide completion and validation.

The active profile is selected by `activeProfile`. A runtime can be a bundled
Stock/BeeLlama build, a local executable, or a verified HTTPS archive. Download
sources require a 64-character SHA-256 digest and are installed only by an
explicit installer action.

Myelin keeps the last successful configuration in `ai-config.applied.json`.
Editing the user file does not interrupt a running turn. Use Settings → AI
Configuration → Validate, then Apply. If validation or startup fails, the
currently applied runtime remains active and the error is shown.

Primary runtimes must implement the llama-server health and OpenAI-compatible
streaming endpoints plus `cache_prompt`, `id_slot`, and the slot save/restore
actions. This requirement is intentional: a runtime that cannot restore KV
files cannot provide Myelin's prepared Chat and Write performance guarantees.

Section KV snapshots are isolated by runtime fingerprint and model fingerprint.
Changing a binary, its relevant libraries, launch arguments, or model creates a
different namespace and cannot restore foreign KV data. The global section
cache budget remains 8 GiB.

For a custom binary, use a profile like the example file, validate it, and then
apply it. If the runtime is a downloaded package, keep the complete extracted
directory so sibling shared libraries remain available.
