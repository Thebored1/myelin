# Pinned live sidecar tests

Pull requests and pushes to `main` run the live sidecar suite on a runner with
the labels `self-hosted`, `linux`, and `myelin-live`.

Configure these repository variables on that runner:

- `MYELIN_LIVE_LLAMA_BIN`: absolute path to the pinned `llama-server` binary.
- `MYELIN_LIVE_LLAMA_SHA256`: SHA-256 of that exact binary.
- `MYELIN_LIVE_MODEL`: absolute path to the pinned LFM2-8B Q2 model.
- `MYELIN_LIVE_MODEL_SHA256`: SHA-256 of that exact model.

The workflow verifies both digests before starting `llama-server`. It then uses
the fixed loopback port `39300`, context size `8192`, eight threads, one parallel
slot, the checked-in LFM2 chat template, and reasoning disabled. The live test
asserts protocol behavior and tool safety rather than exact wording or latency:
plain chat, a nonempty write, read-only search/read routing, bounded mutation,
cancellation with recoverable completion, and clean sidecar/model shutdown.

To reproduce the suite locally when the pinned assets are available:

```sh
LLAMA_URL=http://127.0.0.1:39300/v1 \
  cargo test --manifest-path src-tauri/openharn-myelin/Cargo.toml \
  --test live -- --nocapture
```

The test intentionally skips without `LLAMA_URL` outside CI; CI treats an
unreachable server as a failure.
