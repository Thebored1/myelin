<script lang="ts">
	import { invoke } from '@tauri-apps/api/core';
	import type { SettingsController } from '$lib/settings/controller.svelte';

	let { settings, activeSection }: { settings: SettingsController; activeSection?: string } =
		$props();
</script>

{#if !activeSection || activeSection === 'settings-assistant'}
	<section id="settings-assistant" class="settings-section">
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
					<span class="info" role="note" aria-label="About per-message tool gating">
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
					<span class="info" role="note" aria-label="About deterministic format and find">
						<span class="info-dot">i</span>
						<span class="info-pop"
							>In-code correctness assists that don’t withhold tools — they make the result
							reliable: formatting (strip headings/bold/bullets, change case, convert lists) is
							applied by exact rules instead of the model rewriting the whole note; exact-word
							lookups use a reliable search; and a guard prevents accidentally wiping a note during
							an edit. <strong>On by default.</strong> (Surgical deletes — remove a paragraph/heading/section
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
{/if}

{#if !activeSection || activeSection === 'settings-agent'}
	<section id="settings-agent" class="settings-section">
		<h2>Agent (openharn)</h2>
		<p class="description">
			Myelin runs the openharn agent harness as a sidecar process that drives the local model and
			calls back into Myelin for the real note / search / web tools. These settings tune that
			sidecar. The sidecar binary is required for AI agent and tool-calling features. Run <code
				>npm run build:sidecar</code
			> before development or packaging, or choose an existing binary below. Leave the path blank to use
			the bundled/resource-dir lookup.
		</p>

		<div class="input-group full-width">
			<label for="oh_tool_mode">Tool-calling strategy</label>
			<select id="oh_tool_mode" bind:value={settings.ohToolMode} onchange={settings.changeToolMode}>
				<option value="auto">Auto — choose per request</option>
				<option value="native">Native — use the model's function calls</option>
				<option value="prompt">Prompt tools — text-form calls with grammar options</option>
			</select>
			<p class="compute-hint">
				Auto uses native calls for simple requests and prompt tools only when Openharn's per-request
				policy needs them. Native is usually best for larger models. Prompt tools can help smaller
				or unreliable models, but may reduce quality on larger models.
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
					>Prevents prose when a request is classified as an operation. Useful for weak models; can
					be too restrictive for larger models.</span
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
				<select
					id="oh_tool_choice"
					bind:value={settings.ohToolChoice}
					onchange={settings.saveOpenharn}
				>
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
			<button class="browse-btn" onclick={settings.pickOpenharnBin} disabled={settings.ohSaving}>
				Browse…
			</button>
		</div>
		<p class="compute-hint">
			Explicit path to the sidecar binary. Blank = bundled/resource-dir lookup. If it is missing,
			run <code>npm run build:sidecar</code>
			or set <code>OPENHARN_MYELIN_BIN</code>.
		</p>

		<p class="compute-hint">
			Tool-calling format, intent detection, and reasoning behavior are selected automatically per
			request from the active model profile and interaction mode.
		</p>
	</section>
{/if}
