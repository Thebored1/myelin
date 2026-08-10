<script lang="ts">
import '$lib/settings/settings.css';
import { invoke } from '@tauri-apps/api/core';
import { theme, toggleTheme } from '$lib/theme';
import { chatSidebarShortcut } from '$lib/stores';
import { prettyShortcut } from '$lib/keyboardShortcut';
import { createSettingsController } from '$lib/settings/controller.svelte';
import type { BackendPref } from '$lib/settings/types';

const settings = createSettingsController();
</script>


<div class="settings-container">
	<header class="settings-header">
		<button class="back-btn" onclick={settings.goBack}>
			<svg
				viewBox="0 0 24 24"
				width="20"
				height="20"
				stroke="currentColor"
				stroke-width="2"
				fill="none"
				stroke-linecap="round"
				stroke-linejoin="round"
			>
				<line x1="19" y1="12" x2="5" y2="12"></line>
				<polyline points="12 19 5 12 12 5"></polyline>
			</svg>
			Back to Notes
		</button>
		<h1>Settings</h1>
	</header>

	<div class="settings-content">
		<section class="settings-section">
			<h2>Workspace</h2>
			<div class="info-grid">
				<div class="info-card">
					<span class="info-label">Path</span>
					<span class="info-value">{settings.activeWorkspacePath || '—'}</span>
				</div>
				<div class="info-card">
					<span class="info-label">Index</span>
					<span class="info-value"
						>{settings.indexState ? `${settings.indexState.backend}:${settings.indexState.noteCount} notes` : '—'}</span
					>
				</div>
				<div class="info-card">
					<span class="info-label">Provider</span>
					<span class="info-value">{settings.activeProvider || '—'}</span>
				</div>
			</div>
			<div class="ws-actions">
				<button class="browse-btn" onclick={settings.changeWorkspace}>Change workspace</button>
				<button class="browse-btn" onclick={settings.rebuildIndex} disabled={settings.isRebuilding}>
					{settings.isRebuilding ? 'Rebuilding…' : 'Rebuild index'}
				</button>
			</div>
		</section>

		<section class="settings-section">
			<h2>AI Configuration File</h2>
			<p class="description">
				Technical AI settings are managed in a versioned JSON file so custom llama-server runtimes
				and model profiles can be changed without adding fragile UI controls.
			</p>
			<div class="info-grid">
				<div class="info-card"><span class="info-label">Status</span><span class="info-value">{settings.aiConfig?.validationState ?? '—'}</span></div>
				<div class="info-card"><span class="info-label">Profile</span><span class="info-value">{settings.aiConfig?.activeProfile ?? '—'}</span></div>
				<div class="info-card"><span class="info-label">Runtime</span><span class="info-value">{settings.aiConfig?.runtimeId ?? '—'}</span></div>
			</div>
			<p class="compute-hint">{settings.aiConfig?.configPath ?? 'AI config path unavailable'}{settings.aiConfig?.hasUnappliedChanges ? ' · unapplied changes' : ''}</p>
			{#if settings.aiConfig?.errors?.length}
				<div class="error-box">{#each settings.aiConfig.errors as error}<div><code>{error.path}</code>: {error.message}</div>{/each}</div>
			{/if}
			{#if settings.aiConfigMessage}<p class="compute-hint">{settings.aiConfigMessage}</p>{/if}
			<div class="ws-actions">
				<button class="browse-btn" onclick={settings.openAiConfig} disabled={!settings.aiConfig}>Open config</button>
				<button class="browse-btn" onclick={settings.copyAiConfigPath} disabled={!settings.aiConfig}>Copy path</button>
				<button class="browse-btn" onclick={settings.validateAiConfig} disabled={settings.aiConfigBusy}>Validate</button>
				<button class="browse-btn" onclick={settings.applyAiConfig} disabled={settings.aiConfigBusy || !settings.aiConfig?.candidateHash || settings.aiConfig.validationState !== 'valid'}>Apply</button>
			</div>
		</section>

		<section class="settings-section">
			<h2>External AI Provider</h2>
			<p class="description">
				Use a hosted or separately running OpenAI-compatible model for simple Chat and Write requests.
				This is independent of the local runtime JSON configuration and does not use Myelin's llama.cpp
				slots or section KV caches.
			</p>
			<label class="toggle-row">
				<input type="checkbox" bind:checked={settings.externalEnabled} onchange={settings.saveOpenharn} />
				<span class="toggle-text">
					<strong>Use external model for Chat and Write</strong>
					<span class="toggle-hint">Turn off to return to the active local model profile.</span>
				</span>
			</label>
			<div class="advanced-grid">
				<div class="input-group">
					<label for="external_base_url">API base URL</label>
					<input
						type="url"
						id="external_base_url"
						bind:value={settings.externalBaseUrl}
						onchange={settings.saveOpenharn}
						placeholder="https://api.openai.com/v1"
					/>
				</div>
				<div class="input-group">
					<label for="external_model">Model name</label>
					<input
						type="text"
						id="external_model"
						bind:value={settings.externalModel}
						onchange={settings.saveOpenharn}
						placeholder="gpt-4o-mini or local-model-name"
					/>
				</div>
				<div class="input-group full-width">
					<label for="external_api_key">API key</label>
					<input
						type="password"
						id="external_api_key"
						bind:value={settings.externalApiKey}
						onchange={settings.saveOpenharn}
						placeholder="Optional for local compatible servers"
						autocomplete="off"
					/>
					<p class="compute-hint">Stored locally in Myelin's settings.json and omitted from debug prompts.</p>
				</div>
			</div>
			<p class="compute-hint">
				The base URL should normally end in <code>/v1</code>. Chat works with text-only providers; Write
				requires compatible tool/function calling.
			</p>
		</section>

		<div style="display: none">
		{#if true}
		<section class="settings-section">
			<h2>Local AI Model Configuration</h2>
			<p class="description">
				Select a model to use for local AI features. It runs completely offline on your device and
				is saved in app settings, not inside the notes workspace. <strong
					>Only <code>.gguf</code> models are supported</strong
				> (llama.cpp format).
			</p>

			<div class="model-picker">
				<div class="path-display" class:empty={!settings.currentModelPath}>
					{settings.currentModelPath || 'No model selected'}
				</div>
				<button class="browse-btn" onclick={settings.selectModel} disabled={settings.isSaving}> Browse... </button>
			</div>

			<div class="compute-device">
				<span class="compute-label">Inference engine</span>
				<div class="segmented" role="group" aria-label="Inference engine">
					<button
						type="button"
						class="segment"
						class:active={settings.inferenceEngine === 'llama_cpp'}
						onclick={() => settings.selectInferenceEngine('llama_cpp')}
					>
						llama.cpp · Stable
					</button>
					<button
						type="button"
						class="segment"
						class:active={settings.inferenceEngine === 'beellama'}
						disabled={settings.beeDownloadActive}
						onclick={() => settings.selectInferenceEngine('beellama')}
					>
						{settings.beeDownloadActive
							? 'Installing BeeLlama…'
							: settings.installedBeeBackends.includes(settings.recommendedBeeBackend)
								? 'BeeLlama · Experimental'
								: `Download BeeLlama ${settings.backendLabel(settings.recommendedBeeBackend)}`}
					</button>
				</div>
				<p class="compute-hint">
					BeeLlama v0.4.1 is an experimental, drop-in llama-server fork. Prompt and persistent slot
					caching remain enabled. Stock llama.cpp is used automatically if Bee fails to start.
				</p>
				{#if settings.inferenceEngine === 'beellama'}
					<div class="device-issue warn">
						<span class="issue-icon">⚠️</span>
						<span>
							Experimental engine selected.
							{settings.activeEngine === 'llama_cpp'
								? ' BeeLlama failed or is unavailable, so the stable engine is active.'
								: ` Active backend: ${settings.activeBackend?.toUpperCase() ?? 'not started yet'}.`}
						</span>
					</div>
				{/if}
			</div>

			<div class="compute-device">
				<span class="compute-label">Compute device</span>
				<div class="segmented" role="group" aria-label="Compute device">
					{#each [{ value: 'auto', label: 'Auto' }, { value: 'cuda', label: 'CUDA' }, { value: 'vulkan', label: 'Vulkan' }, ...(settings.downloadableBackends.includes('metal') ? [{ value: 'metal', label: 'Metal' }] : []), { value: 'cpu', label: 'CPU' }] as opt}
						{@const disabled = opt.value !== 'auto' && opt.value !== 'cpu' && !settings.installedBackends.includes(opt.value)}
						<button
							type="button"
							class="segment"
							class:active={settings.backendPreference === opt.value}
							{disabled}
							title={disabled ? `No ${opt.label} build installed` : ''}
							onclick={() => settings.selectBackend(opt.value as BackendPref)}
						>
							{opt.label}
						</button>
					{/each}
				</div>
				<p class="compute-hint">
					{#if settings.backendPreference === 'cpu'}
						Most reliable: runs entirely on the CPU. Works with every model (some models give wrong
						output on the GPU), at the cost of speed.
					{:else if settings.backendPreference === 'vulkan'}
						Power-saving: runs on the integrated GPU via Vulkan. The app still manages offload and
						falls back to CPU if needed.
					{:else if settings.backendPreference === 'cuda'}
						NVIDIA GPU acceleration through CUDA. Requires the installed CUDA backend.
					{:else if settings.backendPreference === 'metal'}
						macOS GPU acceleration: runs through Apple Metal. Requires the downloaded Metal backend
						and falls back to CPU if needed.
					{:else}
						Recommended: detects your hardware and picks the fastest backend — dedicated GPU (CUDA),
						integrated GPU (Vulkan), or CPU — and falls back on its own.
					{/if}
				</p>

				{#if settings.gpuIssue}
					<div class="device-issue warn">
						<span class="issue-icon">⚠️</span>
						<span>{settings.gpuIssue.message}</span>
					</div>
				{/if}

				{#if !settings.providerHealthy && settings.providerDetail}
					<div class="device-issue error">
						<span class="issue-icon">⛔</span>
						<span>{settings.providerDetail}</span>
					</div>
				{/if}
			</div>

			<div
				class="backend-status"
				class:gpu={settings.computeStatus.level === 'gpu'}
				class:cpu={settings.computeStatus.level === 'cpu'}
			>
				<span class="backend-dot"></span>
				<div class="backend-text">
					<strong>{settings.computeStatus.title}</strong>
					<span>{settings.computeStatus.detail}</span>
				</div>
			</div>

			{#if settings.downloadableBackends.length > 0}
				<div class="backends-list">
					<span class="compute-label">Installed backends</span>
					{#each settings.downloadableBackends as b}
						{@const installed = settings.installedBackends.includes(b)}
						{@const busy =
							settings.download?.backend === b && settings.download?.phase !== 'done' && settings.download?.phase !== 'error'}
						<div class="backend-item">
							<span class="backend-name">{settings.backendLabel(b)}</span>
							{#if busy}
								<div class="backend-progress">
									<div class="backend-bar">
										<div class="backend-bar-fill" style="width:{settings.download?.percent ?? 0}%"></div>
									</div>
									<span class="backend-progress-text"
										>{settings.download?.message ?? ''} ({Math.round(settings.download?.percent ?? 0)}%)</span
									>
								</div>
							{:else if installed}
								<span class="backend-installed">✓ Installed</span>
							{:else}
								<button
									class="browse-btn"
									onclick={() => settings.downloadBackend(b)}
									disabled={!!settings.download && settings.download.phase !== 'done' && settings.download.phase !== 'error'}
								>
									Download
								</button>
							{/if}
						</div>
					{/each}
					{#if settings.download?.phase === 'error'}
						<div class="device-issue error">
							<span class="issue-icon">⛔</span>
							<span>Download failed: {settings.download.message}</span>
						</div>
					{/if}
					<p class="compute-hint">
						CPU and Vulkan ship with the app where supported. On macOS, download Metal for Apple GPU
						acceleration; on Windows, download CUDA for NVIDIA GPUs.
					</p>
				</div>
			{/if}

			<br />
			<h2>Web Search</h2>
			<p class="description">
				Privacy-first web search for the assistant. Set a SearXNG instance URL, or leave blank to
				use the no-key DuckDuckGo fallback.
			</p>
			<div class="model-picker">
				<input
					type="text"
					class="path-display"
					bind:value={settings.searxngUrl}
					placeholder="https://searx.example.org (optional)"
					onchange={settings.saveSearxng}
				/>
			</div>

			<br />
			<h2>Embeddings — Document Search & RAG</h2>
			<p class="description">
				A small embedding model (e.g. nomic-embed-text v1.5, Q8_0) powers semantic search over your
				notes and lets the assistant search ingested documents (PDFs, books). It runs as a second
				local server.
			</p>
			<div class="model-picker">
				<div class="path-display" class:empty={!settings.embedModelPath}>
					{settings.embedModelPath ||
						'No embedding model selected — semantic search uses a lexical fallback'}
				</div>
				<button class="browse-btn" onclick={settings.pickEmbedModel}>Browse...</button>
				{#if settings.embedModelPath}
					<button class="browse-btn" onclick={settings.clearEmbedModel}>Clear</button>
				{/if}
			</div>

			{#if settings.modelProfiles.length > 0}
				<br />
				<h2>Compatible Models</h2>
				<p class="description">
					Models with a verified profile work out of the box (tool-calling and chat template tuned).
					Other models run on auto-detected defaults.
				</p>
				<div class="backends-list">
					{#each settings.modelProfiles as p}
						<div class="backend-item">
							<span class="backend-name">
								{p.name}{#if p.role === 'embed'}
									<small>(embedding)</small>{/if}
							</span>
							{#if p.verified}
								<span class="backend-installed">✓ Verified</span>
							{:else}
								<span class="backend-progress-text">Experimental</span>
							{/if}
						</div>
						{#if p.notes}
							<p class="compute-hint">{p.notes}</p>
						{/if}
					{/each}
				</div>
			{/if}

			<br />
			<h2>Advanced AI Configuration</h2>
			<p class="description">
				Fine-tune llama-server memory usage and CLI flags. Leave blank to use system defaults.
			</p>
			<label class="toggle-row">
				<input
					type="checkbox"
					bind:checked={settings.promptCache}
					onchange={async () => {
						await invoke('set_prompt_cache', { enabled: settings.promptCache });
						settings.llamaCache = await invoke('llama_cache_status');
					}}
				/>
				<span class="toggle-text">
					<strong>Persistent per-note prompt cache</strong>
					<span class="toggle-hint">
						Reuses llama.cpp slot snapshots across restarts. No automatic retention limit.
						{#if settings.llamaCache}
							({settings.formatMB(settings.llamaCache.sizeBytes)} cached){/if}
					</span>
				</span>
			</label>
			<button
				class="secondary"
				onclick={async () => {
					await invoke('clear_llama_cache');
					settings.llamaCache = await invoke('llama_cache_status');
				}}>Clear prompt cache</button
			>
			<label class="toggle-row">
				<input type="checkbox" bind:checked={settings.autoOffload} onchange={settings.debounceSave} />
				<span class="toggle-text">
					<strong>Adaptive GPU offload (recommended)</strong>
					<span class="toggle-hint">
						{settings.autoOffload
							? 'On — automatically uses available VRAM, keeps the KV cache in RAM for a large (32k) context, and retries with less if the GPU runs out. Manages Context Size & GPU Layers for you.'
							: 'Off — use the manual Context Size & GPU Layers below exactly as set.'}
					</span>
				</span>
			</label>

			<div class="advanced-grid">
				<div class="input-group">
					<label for="ctx">Context Size {settings.autoOffload ? '(auto)' : ''}</label>
					<input
						type="number"
						id="ctx"
						bind:value={settings.contextSize}
						oninput={settings.debounceSave}
						placeholder="auto"
						disabled={settings.autoOffload}
					/>
				</div>
				<div class="input-group">
					<label for="ngl">GPU Layers {settings.autoOffload ? '(auto)' : ''}</label>
					<input
						type="number"
						id="ngl"
						bind:value={settings.gpuLayers}
						oninput={settings.debounceSave}
						placeholder="auto"
						disabled={settings.autoOffload}
					/>
				</div>
				<div class="input-group">
					<label for="threads">CPU Threads</label>
					<input
						type="number"
						id="threads"
						bind:value={settings.threads}
						oninput={settings.debounceSave}
						placeholder={settings.recommendedThreads
							? `Auto — ${settings.recommendedThreads} physical cores`
							: 'Auto'}
					/>
					<span class="toggle-hint">
						{settings.threads
							? `Explicit override: ${settings.threads} threads`
							: settings.recommendedThreads
								? `Auto uses ${settings.recommendedThreads} physical cores`
								: 'Auto uses the detected physical cores'}
					</span>
				</div>
				<div class="input-group">
					<label for="temp">Temperature</label>
					<input
						type="number"
						step="0.1"
						id="temp"
						bind:value={settings.temperature}
						oninput={settings.debounceSave}
						placeholder="0.2"
					/>
				</div>
				<div class="input-group">
					<label for="top_p">Top P</label>
					<input
						type="number"
						step="0.05"
						id="top_p"
						bind:value={settings.topP}
						oninput={settings.debounceSave}
						placeholder="0.95"
					/>
				</div>
				<div class="input-group">
					<label for="max_turns">Max Tool Turns</label>
					<input
						type="number"
						min="1"
						max="12"
						step="1"
						id="max_turns"
						bind:value={settings.maxTurns}
						oninput={settings.debounceSave}
						placeholder="4"
					/>
				</div>
			</div>

			<label class="toggle-row">
				<input type="checkbox" bind:checked={settings.thinking} onchange={settings.debounceSave} />
				<span class="toggle-text">
					<strong>Model thinking / reasoning</strong>
					<span class="toggle-hint">
						{settings.thinking
							? 'On — the model reasons before answering (slower, may be more accurate).'
							: 'Off — faster, no hidden reasoning tokens. Works across models.'}
					</span>
				</span>
			</label>

			<div class="input-group full-width" style="margin-top: 1rem;">
				<label>
					Extra Arguments
					<div style="font-size: 0.8em; color: var(--text-error); margin-top: 4px;">
						<strong>CRITICAL NOTE:</strong> Because of how system processes work, you cannot put
						spaces in a single box! If you wanted to add <code>--threads 8</code>, you must put
						<code>--threads</code>
						in one box, click add again, and put <code>8</code> in the next box!
					</div>
				</label>
				{#each settings.extraArgs as arg, i}
					<div style="display: flex; gap: var(--space-2); margin-bottom: var(--space-2);">
						<input
							type="text"
							bind:value={settings.extraArgs[i]}
							oninput={settings.debounceSave}
							placeholder="--flash-attn"
							style="flex: 1;"
						/>
						<button
							class="browse-btn"
							onclick={() => settings.removeExtraArg(i)}
							title="Remove argument"
							style="padding: 0 1rem; color: #f87171; border-color: rgba(248, 113, 113, 0.3);"
						>
							Remove
						</button>
					</div>
				{/each}
				<button
					class="browse-btn"
					onclick={settings.addExtraArg}
					style="align-self: flex-start; margin-top: 4px;"
				>
					+ Add Argument
				</button>
			</div>
		</section>

		<section class="settings-section">
			<h2>Assistant Tooling</h2>
			<p class="description">
				Two independent assists for how the model uses tools. Hover each <span class="info-dot"
					>i</span
				> for details and caveats.
			</p>

			<label class="toggle-row">
				<input
					type="checkbox"
					bind:checked={settings.toolGating}
					onchange={() => invoke('set_tool_gating', { enabled: settings.toolGating })}
				/>
				<span class="toggle-text">
					<span class="toggle-label">
						<strong>Per-message tool gating</strong>
						<span class="info" tabindex="0" role="note" aria-label="About per-message tool gating">
							<span class="info-dot">i</span>
							<span class="info-pop"
								>Offers the model only the tools its message seems to need, chosen by keyword
								heuristics — brittle and not model-agnostic. It can <strong
									>withhold a tool the model would have used</strong
								>: e.g. “search for the latest news” isn’t recognised as a web search, so the model
								can’t search at all. <strong>Off by default</strong> — only useful for sub-2B models that
								misfire on tools they shouldn’t touch.</span
							>
						</span>
					</span>
					<span class="toggle-hint">
						{settings.toolGating
							? 'On — only the tools your message seems to need are offered each turn.'
							: 'Off — full toolset every turn, the model decides (recommended).'}
					</span>
				</span>
			</label>

			<label class="toggle-row">
				<input
					type="checkbox"
					bind:checked={settings.deterministicTools}
					onchange={() => invoke('set_deterministic_tools', { enabled: settings.deterministicTools })}
				/>
				<span class="toggle-text">
					<span class="toggle-label">
						<strong>Deterministic format &amp; find</strong>
						<span
							class="info"
							tabindex="0"
							role="note"
							aria-label="About deterministic format and find"
						>
							<span class="info-dot">i</span>
							<span class="info-pop"
								>In-code correctness assists that don’t withhold tools — they make the result
								reliable: formatting (strip headings/bold/bullets, change case, convert lists) is
								applied by exact rules instead of the model rewriting the whole note; exact-word
								lookups use a reliable search; and a guard prevents accidentally wiping a note
								during an edit. <strong>On by default.</strong> (Surgical deletes — remove a paragraph/heading/section
								— always apply, regardless of this toggle.)</span
							>
						</span>
					</span>
					<span class="toggle-hint">
						{settings.deterministicTools
							? 'On — reliable in-code formatting, search & a wipe guard.'
							: 'Off — the model handles formatting & edits on its own.'}
					</span>
				</span>
			</label>
		</section>

		<section class="settings-section">
			<h2>Agent (openharn)</h2>
			<p class="description">
				Myelin runs the openharn agent harness as a sidecar process that drives the local model and
				calls back into Myelin for the real note / search / web tools. These settings tune that
				sidecar. The sidecar binary is required for AI agent and tool-calling features. Run <code
					>npm run build:sidecar</code
				> before development or packaging, or choose an existing binary below. Leave the path blank to
				use the bundled/resource-dir lookup.
			</p>

			<div class="input-group full-width">
				<label for="oh_tool_mode">Tool-calling strategy</label>
				<select id="oh_tool_mode" bind:value={settings.ohToolMode} onchange={settings.changeToolMode}>
					<option value="auto">Auto — choose per request</option>
					<option value="native">Native — use the model's function calls</option>
					<option value="prompt">Prompt tools — text-form calls with grammar options</option>
				</select>
				<p class="compute-hint">
					Auto uses native calls for simple requests and prompt tools only when Openharn's
					per-request policy needs them. Native is usually best for larger models. Prompt tools can
					help smaller or unreliable models, but may reduce quality on larger models.
				</p>
			</div>

			<label class="toggle-row">
				<input
					type="checkbox"
					bind:checked={settings.ohStrict}
					onchange={settings.saveOpenharn}
					disabled={settings.ohToolMode !== 'prompt'}
				/>
				<span class="toggle-text">
					<strong>Strict tool grammar</strong>
					<span class="toggle-hint"
						>Restricts text-form tool calls to valid structured syntax. More reliable, but less
						flexible and slower.</span
					>
				</span>
			</label>

			<label class="toggle-row">
				<input
					type="checkbox"
					bind:checked={settings.ohCallOnly}
					onchange={settings.saveOpenharn}
					disabled={settings.ohToolMode !== 'prompt'}
				/>
				<span class="toggle-text">
					<strong>Call-only tool requests</strong>
					<span class="toggle-hint"
						>Prevents prose when a request is classified as an operation. Useful for weak models;
						can be too restrictive for larger models.</span
					>
				</span>
			</label>

			<label class="toggle-row">
				<input type="checkbox" bind:checked={settings.ohNoThink} onchange={settings.saveOpenharn} />
				<span class="toggle-text">
					<strong>Disable model reasoning</strong>
					<span class="toggle-hint"
						>Adds a request hint to skip hidden thinking tokens. This can make responses faster, but
						some models may become less accurate.</span
					>
				</span>
			</label>

			<div class="advanced-grid">
				<div class="input-group">
					<label for="oh_tool_choice">Native tool choice</label>
					<select id="oh_tool_choice" bind:value={settings.ohToolChoice} onchange={settings.saveOpenharn}>
						<option value="">Auto</option>
						<option value="required">Required — force a native tool call</option>
						<option value="none">None — disable native tool calls</option>
					</select>
				</div>
				<div class="input-group">
					<label for="oh_template_kwargs">Chat-template options (JSON)</label>
					<input
						type="text"
						id="oh_template_kwargs"
						bind:value={settings.ohTemplateKwargs}
						onchange={settings.saveOpenharn}
						placeholder="JSON, e.g. enable_thinking false"
					/>
				</div>
			</div>

			<div class="advanced-grid">
				<div class="input-group">
					<label for="oh_port">Sidecar port</label>
					<input
						type="number"
						id="oh_port"
						bind:value={settings.ohPort}
						onchange={settings.saveOpenharn}
						placeholder="8091"
					/>
				</div>
				<div class="input-group">
					<label for="oh_maxcalls">Max tool calls / turn</label>
					<input
						type="number"
						min="1"
						id="oh_maxcalls"
						bind:value={settings.ohMaxCalls}
						onchange={settings.saveOpenharn}
						placeholder="auto"
					/>
				</div>
				<div class="input-group">
					<label for="oh_totalmax">Max tool calls / chat</label>
					<input
						type="number"
						min="1"
						id="oh_totalmax"
						bind:value={settings.ohTotalMax}
						onchange={settings.saveOpenharn}
						placeholder="auto"
					/>
				</div>
				<div class="input-group">
					<label for="oh_timeout">Tool timeout (s)</label>
					<input
						type="number"
						min="1"
						id="oh_timeout"
						bind:value={settings.ohToolTimeout}
						onchange={settings.saveOpenharn}
						placeholder="300"
					/>
				</div>
			</div>

			<div class="model-picker">
				<input
					type="text"
					class="path-display"
					bind:value={settings.ohBaseUrl}
					placeholder="llama-server base URL override, e.g. http://127.0.0.1:39281/v1"
					onchange={settings.saveOpenharn}
				/>
			</div>
			<p class="compute-hint">
				Override the llama-server URL the sidecar calls. Blank = the model server Myelin is already
				configured to use.
			</p>

			<div class="model-picker">
				<div class="path-display" class:empty={!settings.ohBinPath}>
					{settings.ohBinPath || 'Bundled openharn-myelin (auto-detected)'}
				</div>
				<button class="browse-btn" onclick={settings.pickOpenharnBin} disabled={settings.ohSaving}> Browse… </button>
			</div>
			<p class="compute-hint">
				Explicit path to the sidecar binary. Blank = bundled/resource-dir lookup. If it is missing,
				run <code>npm run build:sidecar</code> or set <code>OPENHARN_MYELIN_BIN</code>.
			</p>

			<p class="compute-hint">
				Tool-calling format, intent detection, and reasoning behavior are selected automatically per
				request from the active model profile and interaction mode.
			</p>
		</section>

		{/if}
		</div>
		<section class="settings-section">
			<h2>Appearance</h2>
			<div
				class="feature-toggle"
				style="display: flex; justify-content: space-between; align-items: center; margin-top: 1rem;"
			>
				<div>
					<h3 style="margin: 0; font-size: 1rem;">Theme</h3>
					<p class="description" style="margin-top: 4px;">
						Switch between the dark and light interface. Your choice is remembered across sessions.
					</p>
				</div>
				<button class="browse-btn" onclick={toggleTheme}>
					{$theme === 'light' ? 'Light' : 'Dark'}
				</button>
			</div>
		</section>

		<section class="settings-section">
			<h2>Features</h2>
			<div
				class="feature-toggle"
				style="display: flex; justify-content: space-between; align-items: center; margin-top: 1rem;"
			>
				<div>
					<h3 style="margin: 0; font-size: 1rem;">Jupyter Code Execution</h3>
					<p class="description" style="margin-top: 4px;">
						Runs Python code cells in <code>.ipynb</code> notebooks in-browser via Pyodide
						(WebAssembly). The core runtime (~14&nbsp;MB) is included with Myelin and initializes on
						first execution. Third-party Python packages requested by a notebook may require a
						network connection.
					</p>
				</div>
				<button class="browse-btn" onclick={settings.toggleJupyterExecution}>
					{settings.enableJupyterExecution ? 'Enabled' : 'Disabled'}
				</button>
			</div>
		</section>

		<section class="settings-section">
			<h2>Keyboard Shortcuts</h2>
			<div
				class="feature-toggle"
				style="display: flex; justify-content: space-between; align-items: center; margin-top: 1rem; gap: 1rem;"
			>
				<div>
					<h3 style="margin: 0; font-size: 1rem;">Chat sidebar</h3>
					<p class="description" style="margin-top: 4px;">
						{#if settings.chatShortcutError}
							<span style="color: var(--danger, #e5534b);">{settings.chatShortcutError}</span>
						{:else if settings.chatShortcutRecording}
							Press your shortcut… (Esc to cancel)
						{:else}
							Current: <strong>{prettyShortcut($chatSidebarShortcut)}</strong>
						{/if}
					</p>
				</div>
				<button
					class="browse-btn"
					onclick={settings.startChatShortcutRecording}
					disabled={settings.chatShortcutRecording}
				>
					{settings.chatShortcutRecording ? 'Recording…' : 'Change'}
				</button>
			</div>
		</section>

		<section class="settings-section">
			<h2>Quick Capture</h2>
			<p class="description">
				A global shortcut opens a small window to jot down a task from anywhere — even when Myelin
				isn't focused.
			</p>
			<div
				class="feature-toggle"
				style="display: flex; justify-content: space-between; align-items: center; margin-top: 1rem; gap: 1rem;"
			>
				<div>
					<h3 style="margin: 0; font-size: 1rem;">Global shortcut</h3>
					<p class="description" style="margin-top: 4px;">
						{#if settings.quickShortcutError}
							<span style="color: var(--danger, #e5534b);">{settings.quickShortcutError}</span>
						{:else if settings.quickRecording}
							Press your shortcut… (Esc to cancel)
						{:else}
							Current: <strong>{prettyShortcut(settings.quickShortcut)}</strong>
						{/if}
					</p>
				</div>
				<button class="browse-btn" onclick={settings.startRecording} disabled={settings.quickRecording}>
					{settings.quickRecording ? 'Recording…' : 'Change'}
				</button>
			</div>
		</section>

		<section class="settings-section">
			<h2>Background</h2>
			<div
				class="feature-toggle"
				style="display: flex; justify-content: space-between; align-items: center; margin-top: 1rem; gap: 1rem;"
			>
				<div>
					<h3 style="margin: 0; font-size: 1rem;">Start Myelin with the system</h3>
					<p class="description" style="margin-top: 4px;">
						Starts hidden in the tray with the shortcut and model ready.
					</p>
					{#if settings.backgroundError}<p class="description" style="color: var(--danger, #e5534b);">
							{settings.backgroundError}
						</p>{/if}
				</div>
				<button
					class="browse-btn"
					onclick={async () => {
						const next = !settings.startWithSystem;
						try {
							await invoke('set_start_with_system', { enabled: next });
							settings.startWithSystem = next;
							settings.backgroundError = '';
						} catch (e) {
							settings.backgroundError = String(e);
						}
					}}
				>
					{settings.startWithSystem ? 'Enabled' : 'Disabled'}
				</button>
			</div>
		</section>

		<section class="settings-section">
			<h2>LaTeX → PDF</h2>
			<p class="description">
				Compiling <code>.tex</code> notes to PDF uses Tectonic, which downloads a LaTeX support bundle
				(~50&nbsp;MB) on first use. Download it now to make the first compile instant and to work fully
				offline afterwards.
			</p>
			<div
				class="feature-toggle"
				style="display: flex; justify-content: space-between; align-items: center; margin-top: 1rem; gap: 1rem;"
			>
				<div>
					<h3 style="margin: 0; font-size: 1rem;">LaTeX support files</h3>
					<p class="description" style="margin-top: 4px;">
						{#if settings.latexDownloading}
							Downloading… {settings.formatMB(settings.latexDownloadBytes)}
						{:else if settings.latexError}
							<span style="color: var(--danger, #e5534b);">Error: {settings.latexError}</span>
						{:else if settings.latexCache?.warmed}
							Ready — {settings.formatMB(settings.latexCache.sizeBytes)} cached.
						{:else}
							Not downloaded yet.
						{/if}
					</p>
				</div>
				<button
					class="browse-btn"
					onclick={settings.downloadLatexSupport}
					disabled={settings.latexDownloading || settings.latexCache?.warmed}
				>
					{#if settings.latexDownloading}
						Downloading…
					{:else if settings.latexCache?.warmed}
						Downloaded
					{:else}
						Download now
					{/if}
				</button>
			</div>
		</section>
	</div>
</div>

	<svelte:window onkeydown={settings.handleAiConfigKeydown} />

{#if settings.showAiConfig}
	<div class="config-modal-backdrop" role="presentation" onclick={(event) => { if (event.target === event.currentTarget) settings.showAiConfig = false; }}>
		<section class="config-modal" role="dialog" aria-modal="true" aria-labelledby="ai-config-title">
			<header class="config-modal-header">
				<div><h2 id="ai-config-title">AI Configuration</h2><p>{settings.aiConfig?.configPath}</p></div>
				<button class="icon-btn" aria-label="Close configuration" onclick={() => (settings.showAiConfig = false)}>×</button>
			</header>
			<div class="config-search-row">
				<input class="config-search" bind:this={settings.aiConfigSearchInput} bind:value={settings.aiConfigSearch} oninput={() => (settings.aiConfigSearchIndex = -1)} onkeydown={settings.handleConfigSearchKeydown} placeholder="Search configuration (Ctrl+F)" aria-label="Search configuration" />
				{#if settings.aiConfigSearch}<span>{settings.configMatchCount() ? `${settings.aiConfigSearchIndex + 1} / ${settings.configMatchCount()}` : '0 matches'}</span>{/if}
				<button class="config-nav-btn" onclick={() => settings.gotoConfigMatch(-1)} disabled={!settings.configMatchCount()} aria-label="Previous match">↑</button>
				<button class="config-nav-btn" onclick={() => settings.gotoConfigMatch(1)} disabled={!settings.configMatchCount()} aria-label="Next match">↓</button>
			</div>
			<textarea class="config-editor" bind:this={settings.aiConfigEditor} bind:value={settings.aiConfigText} spellcheck="false" aria-label="AI configuration JSON"></textarea>
			<div class="config-modal-actions">
				<button class="browse-btn" onclick={() => (settings.showAiConfig = false)}>Cancel</button>
				<button class="browse-btn" onclick={settings.saveAiConfigText} disabled={settings.aiConfigBusy}>Save</button>
				<button class="browse-btn primary" onclick={async () => { await settings.saveAiConfigText(); await settings.validateAiConfig(); }}>Save &amp; Validate</button>
			</div>
		</section>
	</div>
{/if}

{#if settings.saved}
	<div class="success-message">
		<svg
			xmlns="http://www.w3.org/2000/svg"
			width="16"
			height="16"
			viewBox="0 0 24 24"
			fill="none"
			stroke="currentColor"
			stroke-width="2"
			stroke-linecap="round"
			stroke-linejoin="round"
		>
			<polyline points="20 6 9 17 4 12"></polyline>
		</svg>
		Settings saved successfully!
	</div>
{/if}
