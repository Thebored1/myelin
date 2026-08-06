# External OpenAI-Compatible Models in Myelin

This note records the simple third-party model path added to Myelin. It is
separate from the local llama-server runtime and from the section KV-cache
system.

## Configuration

The settings live in the persisted `OpenharnSettings` object in the app's
`settings.json`:

```json
{
  "external_enabled": true,
  "external_base_url": "https://api.openai.com/v1",
  "external_model": "gpt-4o-mini",
  "external_api_key": "..."
}
```

The Settings page exposes these fields under **Agent (openharn) → External
OpenAI-compatible model**. The base URL is the provider API root; the sidecar
appends `/chat/completions`, so a typical value ends in `/v1`, not in
`/chat/completions`.

Examples include hosted providers, Ollama-style local servers, LM Studio,
llama-server-compatible HTTP servers, and other services that accept the
OpenAI streaming chat-completions shape.

## What external mode changes

External mode is deliberately plain. The upstream request does not include:

- `cache_prompt`;
- `id_slot`;
- llama.cpp slot save/restore operations;
- Myelin section-cache preparation;
- Myelin post-turn KV snapshot writes.

The sidecar still handles the normal Myelin conversation, tool routing, write
authorization, previews, approvals, undo behavior, and target-anchor checks.
Write requests therefore still require an armed cursor or selection. The
provider must support OpenAI-compatible tools/function calling for Write to
work; text-only providers remain suitable for Chat.

When external mode is enabled, Myelin does not start or probe the local llama
runtime for that turn. A local configuration may still exist as the fallback
when external mode is disabled.

## Performance tradeoff

This path removes the local model's prompt-cache and slot overhead, but it also
does not receive Myelin's prepared section speedup. Performance is determined
by the provider's own prompt caching, network latency, model queue, prompt
ingestion speed, and generation speed. For a local document repeatedly viewed
page by page, the local runtime with prepared section snapshots can be faster.

For a hosted or already-running local API server, external mode is useful when
the provider has a better model, when the model is not available as a GGUF, or
when the user wants a simple HTTP integration without configuring another
llama-server binary.

## API keys and safety

The current UI stores the API key locally in `settings.json`. It is not emitted
in model-prompt debug events, but the file should still be protected with the
user's normal filesystem permissions. Do not commit `settings.json`, paste the
key into screenshots, or share diagnostic bundles containing application data.

An endpoint receives the note context and conversation required for the
request. Use a trusted provider and review its retention and training policy
before enabling external mode for sensitive notes.

## Troubleshooting

- **Connection or 404 errors:** verify the base URL and ensure it includes the
  provider's API prefix, commonly `/v1`.
- **Model not found:** use the exact model identifier expected by the provider,
  not a local GGUF filename unless the local server uses that name.
- **401/403:** check the API key and provider permissions.
- **Chat works but Write fails:** the provider likely does not support tools or
  the selected model cannot generate tool calls.
- **Slow first token:** external mode has no Myelin section cache; check
  network, provider queue time, and the remote model's prompt ingestion rate.
