<script lang="ts">
import '$lib/settings/settings.css';
import { invoke } from '@tauri-apps/api/core';
import { chatSidebarShortcut } from '$lib/stores';
import { prettyShortcut } from '$lib/keyboardShortcut';
import { createSettingsController } from '$lib/settings/controller.svelte';
import type { BackendPref } from '$lib/settings/types';
import LocalModelSettings from '$lib/settings/LocalModelSettings.svelte';
import AssistantSettings from '$lib/settings/AssistantSettings.svelte';
import ThemeSettings from '$lib/settings/ThemeSettings.svelte';

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


	<LocalModelSettings settings={settings} />
	<AssistantSettings settings={settings} />

		<ThemeSettings />

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
							<span style="color: var(--danger);">{settings.quickShortcutError}</span>
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
