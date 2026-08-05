# Persistent Section KV Caching: Novelty and Marketing Notes

## Bottom line

“Persist a document KV cache and reuse it” is not novel by itself. Prefix caching, prompt caching, persistent slot snapshots, KV offload, and document-specific cached representations are already established areas.

The potentially differentiated contribution is the combination used by Myelin:

> Section-aware persistent KV-cache orchestration for interactive local and edge SLM document assistants.

That means page/section-granular clean snapshots, a single active llama.cpp slot, universal conversation memory outside the snapshots, exact prompt-template boundary discovery, and profile/provenance validation.

This is a product and systems-positioning assessment, not legal patent clearance.

## Closest prior work

### Prompt Cache

[Prompt Cache: Modular Attention Reuse for Low-Latency Inference](https://arxiv.org/abs/2311.04934) proposed precomputing and storing attention states for reusable text segments, including documents used for document QA. This overlaps strongly with the idea of preparing reusable document states.

### llama.cpp slot persistence

[llama.cpp server documentation](https://github.com/ggml-org/llama.cpp/blob/master/tools/server/README.md) exposes slot save and restore endpoints. The underlying persistence primitive is therefore existing infrastructure, not a Myelin invention.

### Prefix-cache serving systems

[vLLM Automatic Prefix Caching](https://docs.vllm.ai/en/v0.13.0/design/prefix_caching/) hashes prompt-prefix blocks and reuses them across requests. [LMCache](https://docs.lmcache.ai/) extends KV management into persistent and tiered storage, including CPU memory, local SSD, remote storage, and non-prefix reuse.

### Persistent edge/agent KV state

[Agent Memory Below the Prompt](https://arxiv.org/abs/2603.04428) explores persistent quantized KV caches for multiple agents on edge devices, including cold priming, disk restoration, cross-phase context, and large TTFT reductions.

### Hybrid/recurrent models

[Sparse Prefix Caching for Hybrid and Recurrent LLM Serving](https://arxiv.org/abs/2605.05219) directly studies different questions over a long document and exact recurrent-state checkpoints. This makes “fast repeated long-document questions on hybrid/SLM models” an active research area.

### Document-specific learned representations

[Cartridges](https://arxiv.org/abs/2508.17032) uses learned, compressed document-specific KV-like prefixes. This is technically different from Myelin’s exact, unmodified prompt-prefix snapshots, but it shows that document-specific KV representations are already an active direction.

## What may be distinctive in Myelin

The strongest differentiators are not individual primitives but the constrained combination:

1. The document is divided according to the user-visible reading surface: PDF pages, EPUB chapters, HTML buckets, or sections.
2. Every section is prepared ahead of time so page navigation does not trigger a cold prefill.
3. Only one low-memory slot is active at a time; the rest live as validated disk snapshots.
4. Conversation history remains universal and is appended after restoring any section, instead of creating a separate conversation checkpoint for every page.
5. Myelin discovers the exact append boundary by asking the server to render two prompts and taking their common prefix, avoiding duplicated chat-template logic.
6. Cache identities validate model binary, model file, template, context size, backend flags, tools, system prompt, and wire revision.
7. Ordinary active-page Chat is fast and section-grounded, while explicit whole-document or other-page requests use retrieval instead.

The web search did not find this exact end-to-end combination. That is evidence for a potentially interesting systems story, not proof of legal novelty.

## Recommended claims

Good technical positioning:

- “Persistent page-aware KV snapshots for local document assistants.”
- “A clean section state plus universal conversation tail.”
- “Fast page switching for CPU-bound small language models.”
- “Document context that is prepared once, swapped on demand, and kept separate from chat memory.”
- “RAG for discovery; KV snapshots for repeated local reading.”

Avoid claiming:

- that Myelin invented KV caching;
- that it invented prefix caching or disk-backed KV storage;
- that preparation eliminates generation time;
- that one prepared cache works unchanged across Chat, Write, models, templates, or context sizes;
- that the approach is patent-safe without a formal prior-art and patent search.

## Blog angles

### Engineering post

**Why `cached=0` can happen after a successful KV restore**

Explain the shared-sentinel bug, dirty resident slot, prompt-profile mismatch, and why save/restore token counts alone do not prove reuse.

### Architecture post

**The clean section state and the universal conversation tail**

Explain why per-page conversation checkpoints cause branching and why clean section snapshots plus bounded universal memory preserve both speed and cross-page continuity.

### Product post

**Making a CPU-bound SLM feel instant over long documents**

Show the difference between cold prefill, page-cache restore, and generation. Focus on time-to-first-token and page navigation rather than vague “faster AI” claims.

### Technical deep dive

**How to find a chat-template cache boundary without reimplementing Jinja**

Describe rendering two prompts with first-character-distinct sentinels, taking the exact common prefix, and evaluating only that raw prefix.

## Evidence needed for a strong public claim

Benchmark at least:

- cold prefill versus restored section;
- second question on the same page;
- switching among pages after preparation;
- app restart followed by restore;
- CPU versus GPU backends;
- recurrent/hybrid versus transformer models;
- Chat versus Write profiles;
- unchanged section versus edited document invalidation;
- page-local answering versus whole-document RAG;
- restore I/O time, prefill time, TTFT, decode time, memory, and cache size.

Report p50 and p95 values, not only the fastest trace. Also report preparation cost and the number of questions/page switches required to amortize it.

