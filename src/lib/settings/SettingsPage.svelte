<script lang="ts">
	import '$lib/settings/settings.css';
	import { invoke } from '@tauri-apps/api/core';
	import { chatSidebarShortcut } from '$lib/stores';
	import { prettyShortcut } from '$lib/keyboardShortcut';
	import { createSettingsController } from '$lib/settings/controller.svelte';
	import LocalModelSettings from '$lib/settings/LocalModelSettings.svelte';
	import AssistantSettings from '$lib/settings/AssistantSettings.svelte';
	import ThemeSettings from '$lib/settings/ThemeSettings.svelte';

	const settings = createSettingsController();
	let activeSection = $state('settings-workspace');
	let settingsQuery = $state('');
	const settingsGroups = [
		{
			label: 'Workspace',
			items: [{ id: 'settings-workspace', label: 'Workspace', icon: 'folder' }]
		},
		{
			label: 'AI',
			items: [
				{ id: 'settings-ai-config', label: 'Configuration file', icon: 'file' },
				{ id: 'settings-external-ai', label: 'External provider', icon: 'cloud' },
				{ id: 'settings-local-ai', label: 'Local model', icon: 'cpu' },
				{ id: 'settings-web-search', label: 'Web search', icon: 'search' },
				{ id: 'settings-embeddings', label: 'Embeddings & RAG', icon: 'database' },
				{ id: 'settings-reranker', label: 'Reranker', icon: 'list' },
				{ id: 'settings-pdf-ocr', label: 'PDF OCR', icon: 'file' },
				{ id: 'settings-compatible-models', label: 'Compatible models', icon: 'check' },
				{ id: 'settings-advanced-ai', label: 'Advanced AI', icon: 'sliders' },
				{ id: 'settings-assistant', label: 'Assistant tooling', icon: 'sparkles' },
				{ id: 'settings-agent', label: 'Agent', icon: 'bot' }
			]
		},
		{
			label: 'Application',
			items: [
				{ id: 'settings-appearance', label: 'Appearance', icon: 'sun' },
				{ id: 'settings-features', label: 'Features', icon: 'grid' },
				{ id: 'settings-shortcuts', label: 'Keyboard shortcuts', icon: 'keyboard' },
				{ id: 'settings-quick-capture', label: 'Quick capture', icon: 'zap' },
				{ id: 'settings-background', label: 'Background', icon: 'power' },
				{ id: 'settings-latex', label: 'LaTeX → PDF', icon: 'code' }
			]
		}
	] as const;
	const settingsIconPaths: Record<string, string> = {
		folder: 'M2.5 5h6l2 2h7a2 2 0 0 1 2 2v6.5a2 2 0 0 1-2 2h-15z',
		file: 'M6 3h8l4 4v13H6zM14 3v5h5',
		cloud: 'M6 18h11a4 4 0 0 0 .5-7.97A5.5 5.5 0 0 0 7 8.5 4.5 4.5 0 0 0 6 18z',
		cpu: 'M6 6h12v12H6zM9 9h6v6H9zM3 9h3M3 12h3M3 15h3M18 9h3M18 12h3M18 15h3M9 3v3M12 3v3M15 3v3M9 18v3M12 18v3M15 18v3',
		search: 'm21 21-4.35-4.35M10.5 18a7.5 7.5 0 1 1 0-15 7.5 7.5 0 0 1 0 15z',
		database:
			'M4 6c0-1.1 3.6-2 8-2s8 .9 8 2-3.6 2-8 2-8-.9-8-2zM4 6v6c0 1.1 3.6 2 8 2s8-.9 8-2V6M4 12v6c0 1.1 3.6 2 8 2s8-.9 8-2v-6',
		list: 'M7 6h13M7 12h13M7 18h13M3.5 6h.01M3.5 12h.01M3.5 18h.01',
		check: 'm4 12 5 5L20 6',
		sliders: 'M4 6h16M4 12h16M4 18h16M8 4v4M15 10v4M10 16v4',
		sparkles: 'M12 3l1.5 5.5L19 10l-5.5 1.5L12 17l-1.5-5.5L5 10l5.5-1.5zM19 16v5M21.5 18.5h-5',
		bot: 'M7 8h10a2 2 0 0 1 2 2v7H5v-7a2 2 0 0 1 2-2zM12 8V5M9 12h.01M15 12h.01M8 17v2h8v-2',
		sun: 'M12 8a4 4 0 1 0 0 8 4 4 0 0 0 0-8zM12 2v3M12 19v3M4.93 4.93l2.12 2.12M16.95 16.95l2.12 2.12M2 12h3M19 12h3M4.93 19.07l2.12-2.12M16.95 7.05l2.12-2.12',
		grid: 'M4 4h6v6H4zM14 4h6v6h-6zM4 14h6v6H4zM14 14h6v6h-6z',
		keyboard: 'M3 6h18v12H3zM6 10h.01M9 10h.01M12 10h.01M15 10h.01M18 10h.01M6 14h12',
		zap: 'M13 2 4 14h6l-1 8 9-12h-6z',
		power: 'M12 3v9M6.3 6.3a8 8 0 1 0 11.4 0',
		code: 'm9 6-6 6 6 6M15 6l6 6-6 6'
	};
	const filteredSettingsItems = (group: (typeof settingsGroups)[number]) => {
		const query = settingsQuery.trim().toLowerCase();
		return query
			? group.items.filter((item) => `${group.label} ${item.label}`.toLowerCase().includes(query))
			: group.items;
	};
</script>

<div class="settings-container">
	<div class="settings-layout">
		<main class="settings-main">
			<header class="settings-header">
				<h1>Settings</h1>
			</header>

			<div class="settings-content" id="settings-content">
				{#if activeSection === 'settings-workspace'}
					<section id="settings-workspace" class="settings-section">
						<h2>Workspace</h2>
						<div class="info-grid">
							<div class="info-card">
								<span class="info-label">Path</span>
								<span class="info-value">{settings.activeWorkspacePath || '—'}</span>
							</div>
							<div class="info-card">
								<span class="info-label">Index</span>
								<span class="info-value"
									>{settings.indexState
										? `${settings.indexState.backend}:${settings.indexState.noteCount} notes`
										: '—'}</span
								>
							</div>
							<div class="info-card">
								<span class="info-label">Provider</span>
								<span class="info-value">{settings.activeProvider || '—'}</span>
							</div>
						</div>
						<div class="ws-actions">
							<button class="browse-btn" onclick={settings.changeWorkspace}>Change workspace</button
							>
							<button
								class="browse-btn"
								onclick={settings.rebuildIndex}
								disabled={settings.isRebuilding}
							>
								{settings.isRebuilding ? 'Rebuilding…' : 'Rebuild index'}
							</button>
						</div>
					</section>
				{/if}

				{#if activeSection === 'settings-ai-config'}
					<section id="settings-ai-config" class="settings-section">
						<h2>AI Configuration File</h2>
						<p class="description">
							Technical AI settings are managed in a versioned JSON file so custom llama-server
							runtimes and model profiles can be changed without adding fragile UI controls.
						</p>
						<div class="info-grid">
							<div class="info-card">
								<span class="info-label">Status</span><span class="info-value"
									>{settings.aiConfig?.validationState ?? '—'}</span
								>
							</div>
							<div class="info-card">
								<span class="info-label">Profile</span><span class="info-value"
									>{settings.aiConfig?.activeProfile ?? '—'}</span
								>
							</div>
							<div class="info-card">
								<span class="info-label">Runtime</span><span class="info-value"
									>{settings.aiConfig?.runtimeId ?? '—'}</span
								>
							</div>
						</div>
						<p class="compute-hint">
							{settings.aiConfig?.configPath ?? 'AI config path unavailable'}{settings.aiConfig
								?.hasUnappliedChanges
								? ' · unapplied changes'
								: ''}
						</p>
						{#if settings.aiConfig?.errors?.length}
							<div class="error-box">
								{#each settings.aiConfig.errors as error, errorIndex (error.path ?? errorIndex)}<div
									>
										<code>{error.path}</code>: {error.message}
									</div>{/each}
							</div>
						{/if}
						{#if settings.aiConfigMessage}<p class="compute-hint">
								{settings.aiConfigMessage}
							</p>{/if}
						<div class="ws-actions">
							<button
								class="browse-btn"
								onclick={settings.openAiConfig}
								disabled={!settings.aiConfig}>Open config</button
							>
							<button
								class="browse-btn"
								onclick={settings.copyAiConfigPath}
								disabled={!settings.aiConfig}>Copy path</button
							>
							<button
								class="browse-btn"
								onclick={settings.validateAiConfig}
								disabled={settings.aiConfigBusy}>Validate</button
							>
							<button
								class="browse-btn"
								onclick={settings.applyAiConfig}
								disabled={settings.aiConfigBusy ||
									!settings.aiConfig?.candidateHash ||
									settings.aiConfig.validationState !== 'valid'}>Apply</button
							>
						</div>
					</section>
				{/if}

				{#if activeSection === 'settings-external-ai'}
					<section id="settings-external-ai" class="settings-section">
						<h2>External AI Provider</h2>
						<p class="description">
							Use a hosted or separately running OpenAI-compatible model for simple Chat and Write
							requests. This is independent of the local runtime JSON configuration and does not use
							Myelin's llama.cpp slots or section KV caches.
						</p>
						<label class="toggle-row">
							<input
								type="checkbox"
								bind:checked={settings.externalEnabled}
								onchange={settings.saveOpenharn}
							/>
							<span class="toggle-text">
								<strong>Use external model for Chat and Write</strong>
								<span class="toggle-hint"
									>Turn off to return to the active local model profile.</span
								>
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
								<p class="compute-hint">
									Stored locally in Myelin's settings.json and omitted from debug prompts.
								</p>
							</div>
						</div>
						<p class="compute-hint">
							The base URL should normally end in <code>/v1</code>. Chat works with text-only
							providers; Write requires compatible tool/function calling.
						</p>
					</section>
				{/if}

				{#if ['settings-local-ai', 'settings-web-search', 'settings-embeddings', 'settings-reranker', 'settings-pdf-ocr', 'settings-compatible-models', 'settings-advanced-ai'].includes(activeSection)}
					<LocalModelSettings {settings} {activeSection} />
				{/if}

				{#if activeSection === 'settings-assistant' || activeSection === 'settings-agent'}
					<AssistantSettings {settings} {activeSection} />
				{/if}

				{#if activeSection === 'settings-appearance'}
					<ThemeSettings />
				{/if}

				{#if activeSection === 'settings-features'}
					<section id="settings-features" class="settings-section">
						<h2>Features</h2>
						<div
							class="feature-toggle"
							style="display: flex; justify-content: space-between; align-items: center; margin-top: 1rem;"
						>
							<div>
								<h3 style="margin: 0; font-size: 1rem;">Jupyter Code Execution</h3>
								<p class="description" style="margin-top: 4px;">
									Runs Python code cells in <code>.ipynb</code> notebooks in-browser via Pyodide (WebAssembly).
									The core runtime (~14&nbsp;MB) is included with Myelin and initializes on first execution.
									Third-party Python packages requested by a notebook may require a network connection.
								</p>
							</div>
							<button class="browse-btn" onclick={settings.toggleJupyterExecution}>
								{settings.enableJupyterExecution ? 'Enabled' : 'Disabled'}
							</button>
						</div>
					</section>
				{/if}

				{#if activeSection === 'settings-shortcuts'}
					<section id="settings-shortcuts" class="settings-section">
						<h2>Keyboard Shortcuts</h2>
						<div
							class="feature-toggle"
							style="display: flex; justify-content: space-between; align-items: center; margin-top: 1rem; gap: 1rem;"
						>
							<div>
								<h3 style="margin: 0; font-size: 1rem;">Chat sidebar</h3>
								<p class="description" style="margin-top: 4px;">
									{#if settings.chatShortcutError}
										<span style="color: var(--danger);">{settings.chatShortcutError}</span>
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
				{/if}

				{#if activeSection === 'settings-quick-capture'}
					<section id="settings-quick-capture" class="settings-section">
						<h2>Quick Capture</h2>
						<p class="description">
							A global shortcut opens a small window to jot down a task from anywhere — even when
							Myelin isn't focused.
						</p>
						<div
							class="feature-toggle"
							style="display: flex; justify-content: space-between; align-items: center; margin-top: 1rem; gap: 1rem;"
						>
							<div>
								<h3 style="margin: 0; font-size: 1rem;">Global shortcut</h3>
								<p class="description" style="margin-top: 4px;">
									{#if settings.quickShortcutError}
										<span style="color: var(--danger);">{settings.quickShortcutError}</span>
									{:else if settings.quickRecording}
										Press your shortcut… (Esc to cancel)
									{:else}
										Current: <strong>{prettyShortcut(settings.quickShortcut)}</strong>
									{/if}
								</p>
							</div>
							<button
								class="browse-btn"
								onclick={settings.startRecording}
								disabled={settings.quickRecording}
							>
								{settings.quickRecording ? 'Recording…' : 'Change'}
							</button>
						</div>
					</section>
				{/if}

				{#if activeSection === 'settings-background'}
					<section id="settings-background" class="settings-section">
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
								{#if settings.backgroundError}<p class="description" style="color: var(--danger);">
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
				{/if}

				{#if activeSection === 'settings-latex'}
					<section id="settings-latex" class="settings-section">
						<h2>LaTeX → PDF</h2>
						<p class="description">
							Compiling <code>.tex</code> notes to PDF uses Tectonic, which downloads a LaTeX support
							bundle (~50&nbsp;MB) on first use. Download it now to make the first compile instant and
							to work fully offline afterwards.
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
										<span style="color: var(--danger);">Error: {settings.latexError}</span>
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
				{/if}
			</div>
		</main>

		<aside class="settings-sidebar" aria-label="Settings sections">
			<button class="settings-back-btn" type="button" onclick={settings.goBack}>
				<svg
					viewBox="0 0 24 24"
					width="16"
					height="16"
					fill="none"
					stroke="currentColor"
					stroke-width="1.8"
					stroke-linecap="round"
					stroke-linejoin="round"
				>
					<path d="m15 18-6-6 6-6"></path>
				</svg>
				<span>Back to app</span>
			</button>
			<label class="settings-search">
				<svg
					viewBox="0 0 24 24"
					width="15"
					height="15"
					fill="none"
					stroke="currentColor"
					stroke-width="1.8"
					stroke-linecap="round"
				>
					<circle cx="11" cy="11" r="7"></circle>
					<path d="m20 20-4-4"></path>
				</svg>
				<input
					type="search"
					bind:value={settingsQuery}
					placeholder="Search settings..."
					aria-label="Search settings"
				/>
			</label>
			<nav>
				{#each settingsGroups as group (group.label)}
					{@const items = filteredSettingsItems(group)}
					{#if items.length > 0}
						<div class="settings-nav-group">
							<h2>{group.label}</h2>
							{#each items as item (item.id)}
								<button
									type="button"
									class="settings-nav-link"
									class:active={activeSection === item.id}
									aria-current={activeSection === item.id ? 'page' : undefined}
									onclick={() => (activeSection = item.id)}
								>
									<svg
										class="settings-nav-icon"
										viewBox="0 0 24 24"
										width="15"
										height="15"
										fill="none"
										stroke="currentColor"
										stroke-width="1.6"
										stroke-linecap="round"
										stroke-linejoin="round"
										aria-hidden="true"
									>
										<path d={settingsIconPaths[item.icon]}></path>
									</svg>
									{item.label}
								</button>
							{/each}
						</div>
					{/if}
				{/each}
				{#if settingsQuery.trim() && !settingsGroups.some((group) => filteredSettingsItems(group).length)}
					<p class="settings-nav-empty">No matching settings.</p>
				{/if}
			</nav>
		</aside>
	</div>
</div>

<svelte:window onkeydown={settings.handleAiConfigKeydown} />

{#if settings.showAiConfig}
	<div
		class="config-modal-backdrop"
		role="presentation"
		onclick={(event) => {
			if (event.target === event.currentTarget) settings.showAiConfig = false;
		}}
	>
		<div class="config-modal" role="dialog" aria-modal="true" aria-labelledby="ai-config-title">
			<header class="config-modal-header">
				<div>
					<h2 id="ai-config-title">AI Configuration</h2>
					<p>{settings.aiConfig?.configPath}</p>
				</div>
				<button
					class="icon-btn"
					aria-label="Close configuration"
					onclick={() => (settings.showAiConfig = false)}>×</button
				>
			</header>
			<div class="config-search-row">
				<input
					class="config-search"
					bind:this={settings.aiConfigSearchInput}
					bind:value={settings.aiConfigSearch}
					oninput={() => (settings.aiConfigSearchIndex = -1)}
					onkeydown={settings.handleConfigSearchKeydown}
					placeholder="Search configuration (Ctrl+F)"
					aria-label="Search configuration"
				/>
				{#if settings.aiConfigSearch}<span
						>{settings.configMatchCount()
							? `${settings.aiConfigSearchIndex + 1} / ${settings.configMatchCount()}`
							: '0 matches'}</span
					>{/if}
				<button
					class="config-nav-btn"
					onclick={() => settings.gotoConfigMatch(-1)}
					disabled={!settings.configMatchCount()}
					aria-label="Previous match">↑</button
				>
				<button
					class="config-nav-btn"
					onclick={() => settings.gotoConfigMatch(1)}
					disabled={!settings.configMatchCount()}
					aria-label="Next match">↓</button
				>
			</div>
			<textarea
				class="config-editor"
				bind:this={settings.aiConfigEditor}
				bind:value={settings.aiConfigText}
				spellcheck="false"
				aria-label="AI configuration JSON"
			></textarea>
			<div class="config-modal-actions">
				<button class="browse-btn" onclick={() => (settings.showAiConfig = false)}>Cancel</button>
				<button
					class="browse-btn"
					onclick={settings.saveAiConfigText}
					disabled={settings.aiConfigBusy}>Save</button
				>
				<button
					class="browse-btn primary"
					onclick={async () => {
						await settings.saveAiConfigText();
						await settings.validateAiConfig();
					}}>Save &amp; Validate</button
				>
			</div>
		</div>
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
