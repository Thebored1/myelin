<script lang="ts">
	import {
		activeColorTheme,
		activeThemeId,
		availableThemes,
		themeController,
		themeError
	} from '$lib/theme';
	import { applyAccent, cloneTheme } from '$lib/theme/compiler';
	import { contrastChecks } from '$lib/theme/color';
	import { compileTheme } from '$lib/theme/compiler';
	import type { ColorTheme } from '$lib/theme/types';
	import AccentPicker from './AccentPicker.svelte';

	let editing = $state<ColorTheme | null>(null);
	let draftName = $state('');
	let busy = $state(false);
	let message = $state('');
	let importInput = $state<HTMLInputElement | null>(null);
	let accentDraft = $state('#EF6F2E');

	$effect(() => {
		if ($activeColorTheme) {
			accentDraft = compileTheme($activeColorTheme)['accent-100'] ?? '#EF6F2E';
		}
	});

	const paletteFields = [
		['page', 'Page background'],
		['panel', 'Panel background'],
		['text', 'Primary text'],
		['mutedText', 'Muted text'],
		['accent', 'Accent'],
		['danger', 'Danger'],
		['warning', 'Warning'],
		['success', 'Success'],
		['info', 'Info']
	] as const;

	function beginEdit(theme: ColorTheme) {
		const copy = theme.readonly ? cloneTheme(theme) : structuredClone(theme);
		editing = copy;
		draftName = copy.name;
		message = '';
		themeController.preview(copy);
	}

	function applyAccentDraft() {
		const active = $activeColorTheme;
		if (!active) return;
		beginEdit(applyAccent(active, accentDraft));
	}

	function updatePalette(key: (typeof paletteFields)[number][0], value: string) {
		if (!editing) return;
		editing = { ...editing, palette: { ...editing.palette, [key]: value } };
		themeController.preview(editing);
	}

	async function saveDraft() {
		if (!editing) return;
		busy = true;
		try {
			await themeController.save({ ...editing, name: draftName.trim() || 'Custom Theme' });
			editing = null;
			message = 'Theme saved.';
		} catch {
			message = 'Theme could not be saved.';
		} finally {
			busy = false;
		}
	}

	function cancelEdit() {
		editing = null;
		themeController.clearPreview();
	}

	async function selectTheme(id: string) {
		busy = true;
		try {
			await themeController.activate(id);
			message = '';
		} catch {
			message = 'Theme could not be activated.';
		} finally {
			busy = false;
		}
	}

	async function duplicateCurrent() {
		busy = true;
		try {
			const copy = await themeController.duplicate($activeThemeId);
			beginEdit(copy);
		} catch {
			message = 'Theme could not be duplicated.';
		} finally {
			busy = false;
		}
	}

	async function deleteTheme() {
		if (!$activeColorTheme || $activeColorTheme.readonly || !confirm(`Delete ${$activeColorTheme.name}?`)) return;
		busy = true;
		try {
			await themeController.delete($activeColorTheme.id);
			message = 'Theme deleted.';
		} catch {
			message = 'Theme could not be deleted.';
		} finally {
			busy = false;
		}
	}

	function exportCurrent() {
		const current = $activeColorTheme;
		if (!current) return;
		const payload = JSON.stringify({ ...current, readonly: false }, null, 2);
		const link = document.createElement('a');
		link.href = URL.createObjectURL(new Blob([payload], { type: 'application/json' }));
		link.download = `${current.name.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '') || 'myelin-theme'}.myelin-theme.json`;
		link.click();
		URL.revokeObjectURL(link.href);
	}

	async function importFile(event: Event) {
		const input = event.currentTarget as HTMLInputElement;
		const file = input.files?.[0];
		input.value = '';
		if (!file) return;
		busy = true;
		try {
			if (file.size > 256 * 1024) throw new Error('Theme file is too large.');
			const imported = await themeController.importTheme(JSON.parse(await file.text()) as ColorTheme);
			message = `${imported.name} imported.`;
		} catch (error) {
			message = error instanceof Error ? error.message : 'Theme import failed.';
		} finally {
			busy = false;
		}
	}

	const draftContrast = $derived(editing ? contrastChecks(compileTheme(editing)) : []);
</script>

<section class="settings-section theme-settings">
	<h2>Appearance</h2>
	<p class="description">
		Choose a built-in theme or create your own. Custom themes are saved for the whole application and
		shared with the Quick Capture window.
	</p>

	{#if !editing}
		<div class="accent-hero">
			<div class="accent-hero-heading">
				<h3>Accent color</h3>
				<p class="compute-hint">
					Pick one color and the entire app recolors: buttons, links, selection, hovers, focus states,
					and a subtle surface wash. Applied to {$activeColorTheme?.name ?? 'the active theme'}.
				</p>
			</div>
			<AccentPicker value={accentDraft} mode={$activeColorTheme?.mode ?? 'dark'} onchange={(hex) => (accentDraft = hex)} />
			<div class="theme-actions">
				<button
					class="browse-btn primary"
					onclick={() => void applyAccentDraft()}
					disabled={busy}
				>Apply accent</button>
				<span class="accent-hint">Creates an editable copy that you can fine-tune and save below.</span>
			</div>
		</div>
	{/if}

	<div class="theme-gallery">
		{#each $availableThemes as available}
			<button
				class="theme-card"
				class:active={available.id === $activeThemeId}
				onclick={() => void selectTheme(available.id)}
				disabled={busy}
			>
				<span class="theme-swatch" style={`background:${available.tokens['bg-page']}; color:${available.tokens['text-primary']}; border-color:${available.tokens['accent-100']}`}>
					<span style={`background:${available.tokens['accent-100']}`}></span>
					<span style={`background:${available.tokens['bg-panel']}`}></span>
					<span style={`background:${available.tokens['text-primary']}`}></span>
				</span>
				<span class="theme-card-copy">
					<strong>{available.name}</strong>
					<small>{available.readonly ? 'Built-in' : 'Custom'} · {available.mode}</small>
				</span>
			</button>
		{/each}
	</div>

	<div class="theme-actions">
		<button class="browse-btn" onclick={() => void duplicateCurrent()} disabled={busy}>Create editable copy</button>
		<button class="browse-btn" onclick={() => importInput?.click()} disabled={busy}>Import theme</button>
		<button class="browse-btn" onclick={exportCurrent} disabled={busy}>Export active</button>
		<input bind:this={importInput} class="theme-file-input" type="file" accept="application/json,.myelin-theme.json" onchange={(event) => void importFile(event)} />
		{#if $activeColorTheme && !$activeColorTheme.readonly}
			<button class="browse-btn" onclick={() => beginEdit($activeColorTheme)} disabled={busy}>Edit colors</button>
			<button class="browse-btn danger-action" onclick={() => void deleteTheme()} disabled={busy}>Delete</button>
		{/if}
	</div>

	{#if message || $themeError}
		<p class="theme-message" role="status">{message || $themeError}</p>
	{/if}

	{#if editing}
		<div class="theme-editor" aria-label="Custom theme editor">
			<div class="theme-editor-heading">
				<div>
					<h3>Edit custom theme</h3>
					<p class="compute-hint">Changes preview immediately and are not saved until you click Save theme.</p>
				</div>
				<input class="theme-name-input" bind:value={draftName} aria-label="Theme name" maxlength="64" />
			</div>
			<div class="theme-color-grid">
				{#each paletteFields as [key, label]}
					<label class="theme-color-field">
						<span>{label}</span>
						<span class="theme-color-control">
							<input
								type="color"
								value={editing.palette?.[key] ?? '#000000'}
								oninput={(event) => updatePalette(key, event.currentTarget.value)}
							/>
								<code>{editing.palette?.[key] ?? 'derived'}</code>
							</span>
					</label>
				{/each}
			</div>
			<div class="theme-contrast-list">
				{#each draftContrast as check}
					<span class:contrast-fail={!check.pass}>
						{check.foreground} / {check.background}: {check.ratio.toFixed(2)}:1
					</span>
				{/each}
			</div>
			<div class="theme-actions">
				<button class="browse-btn" onclick={cancelEdit} disabled={busy}>Cancel</button>
				<button class="browse-btn primary" onclick={() => void saveDraft()} disabled={busy}>Save theme</button>
			</div>
		</div>
	{/if}
</section>

<style>
	.theme-gallery { display: grid; grid-template-columns: repeat(auto-fit, minmax(180px, 1fr)); gap: .6rem; }
	.accent-hero { margin-top: 1rem; padding: 1rem; border: 1px solid var(--border-default); border-radius: var(--radius-lg); background: var(--bg-elevated); }
	.accent-hero-heading h3 { margin: 0 0 .3rem; }
	.accent-hint { color: var(--text-secondary); font-size: .78rem; align-self: center; }
	.theme-card { display: flex; align-items: center; gap: .65rem; border: 1px solid var(--border-default); border-radius: var(--radius-lg); background: var(--bg-panel); color: var(--text-primary); padding: .55rem; text-align: left; cursor: pointer; }
	.theme-card:hover, .theme-card.active { border-color: var(--accent-100); background: var(--accent-tint); }
	.theme-card:disabled { opacity: .6; cursor: wait; }
	.theme-swatch { display: flex; gap: 3px; align-items: center; width: 54px; height: 34px; padding: 5px; border: 1px solid; border-radius: var(--radius-md); }
	.theme-swatch span { width: 12px; height: 22px; border-radius: 2px; }
	.theme-card-copy { display: grid; gap: .2rem; min-width: 0; }
	.theme-card-copy small { color: var(--text-secondary); font-size: .72rem; }
	.theme-actions { display: flex; flex-wrap: wrap; gap: .5rem; margin-top: .8rem; }
	.theme-file-input { display: none; }
	.danger-action { color: var(--danger-text); }
	.theme-message { color: var(--text-secondary); margin: .7rem 0 0; }
	.theme-editor { margin-top: 1rem; padding: 1rem; border: 1px solid var(--border-default); border-radius: var(--radius-lg); background: var(--bg-elevated); }
	.theme-editor-heading { display: flex; justify-content: space-between; align-items: start; gap: 1rem; }
	.theme-editor-heading h3 { margin: 0; }
	.theme-name-input { width: 12rem; color: var(--text-primary); background: var(--bg-input); border: 1px solid var(--border-default); padding: .45rem; border-radius: var(--radius-md); }
	.theme-color-grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(180px, 1fr)); gap: .65rem; margin-top: 1rem; }
	.theme-color-field { display: grid; gap: .3rem; color: var(--text-secondary); font-size: .78rem; }
	.theme-color-control { display: flex; align-items: center; gap: .4rem; }
	.theme-color-control input { width: 2rem; height: 2rem; padding: 0; border: 0; background: transparent; }
	.theme-color-control code { color: var(--text-primary); font-size: .72rem; }
	.theme-contrast-list { display: flex; flex-wrap: wrap; gap: .5rem .8rem; margin-top: 1rem; color: var(--success); font-size: .72rem; }
	.theme-contrast-list .contrast-fail { color: var(--danger-text); }
	@media (max-width: 600px) { .theme-editor-heading { display: grid; } .theme-name-input { width: 100%; } }
</style>
