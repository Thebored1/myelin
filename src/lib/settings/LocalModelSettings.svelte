<script lang="ts">
	import { invoke } from '@tauri-apps/api/core';

	let { settings }: { settings: any } = $props();
	type BackendPref = 'auto' | 'cuda' | 'vulkan' | 'metal' | 'cpu';
</script>


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
				A validated embedding GGUF powers semantic search over your notes and lets the assistant
				search ingested documents (PDFs, books). Known models receive their required query/document
				format; other compatible models use plain text. It runs as a second local server.
			</p>
			<div class="model-picker">
				<div class="path-display" class:empty={!settings.embedModelPath}>
					{settings.embedModelPath ||
						'No embedding model selected — semantic search uses a lexical fallback'}
				</div>
				<button class="browse-btn" onclick={settings.pickEmbedModel}>Browse...</button>
				<button class="browse-btn" disabled={settings.downloadingModel === 'nomic-embed-text-v1.5-q4-k-m'} onclick={() => settings.downloadBuiltInModel('nomic-embed-text-v1.5-q4-k-m')}>{settings.downloadingModel === 'nomic-embed-text-v1.5-q4-k-m' ? 'Downloading…' : 'Download recommended'}</button>
				{#if settings.embedModelPath}
					<button class="browse-btn" onclick={settings.clearEmbedModel}>Clear</button>
				{/if}
			</div>

			<br />
			<h2>Reranker — Final Evidence Ordering</h2>
			<p class="description">Optional resident English cross-encoder. It only reranks ambiguous agent document retrieval; ordinary note search stays fast and never calls it.</p>
			<div class="model-picker">
				<div class="path-display" class:empty={!settings.rerankerModelPath}>{settings.rerankerModelPath || 'No reranker selected — calibrated hybrid retrieval remains active'}</div>
				<button class="browse-btn" onclick={settings.pickRerankerModel}>Browse...</button>
				<button class="browse-btn" disabled={settings.downloadingModel === 'ms-marco-minilm-l6-v2-q4-k-m'} onclick={() => settings.downloadBuiltInModel('ms-marco-minilm-l6-v2-q4-k-m')}>{settings.downloadingModel === 'ms-marco-minilm-l6-v2-q4-k-m' ? 'Downloading…' : 'Download MiniLM-L6 Q4'}</button>
				{#if settings.rerankerModelPath}<button class="browse-btn" onclick={settings.clearRerankerModel}>Clear</button>{/if}
			</div>
			{#if settings.modelDownloadError}<p class="compute-hint" style="color: var(--text-error)">{settings.modelDownloadError}</p>{/if}

			<br />
			<h2>PDF OCR</h2>
			<p class="description">OCR is reserved for a future bundled runtime. It is currently dormant and native PDF extraction remains active.</p>
			{#if settings.ocrStatus}
				<label class="checkbox-label"><input type="checkbox" checked={settings.ocrStatus.autoLowTextPages ?? true} onchange={(event) => settings.saveOcrSettings({ autoLowTextPages: event.currentTarget.checked, executablePath: settings.ocrStatus.executablePath, language: settings.ocrStatus.configuredLanguage })} disabled /> Automatically OCR low-text pages (inactive until bundled runtime is enabled)</label>
				<div class="model-picker">
					<input class="path-display" value={settings.ocrStatus.executablePath ?? ''} placeholder="Bundled runtime path (future)" onchange={(event) => settings.saveOcrSettings({ autoLowTextPages: settings.ocrStatus.autoLowTextPages ?? true, executablePath: event.currentTarget.value || null, language: settings.ocrStatus.configuredLanguage })} />
					<input class="path-display" value={settings.ocrStatus.configuredLanguage ?? 'eng'} placeholder="eng" onchange={(event) => settings.saveOcrSettings({ autoLowTextPages: settings.ocrStatus.autoLowTextPages ?? true, executablePath: settings.ocrStatus.executablePath, language: event.currentTarget.value })} />
				</div>
				<p class="compute-hint">{settings.ocrStatus.available && settings.ocrStatus.languageAvailable ? `Ready: ${settings.ocrStatus.version ?? 'Tesseract'} (${settings.ocrStatus.configuredLanguage})` : settings.ocrStatus.warning}</p>
			{/if}

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
						style="padding: 0 1rem; color: var(--danger-text); border-color: var(--danger-border);"
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

		{/if}
		</div>
