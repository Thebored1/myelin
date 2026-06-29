<script lang="ts">
	let { notebooks, value = $bindable() } = $props<{ notebooks: string[], value: string | undefined }>();
	
	let isOpen = $state(false);
	let forceClose = $state(false);
	let rootEl: HTMLDivElement | undefined = $state();

	function selectNb(nb: string) {
		value = nb;
		isOpen = false;
		forceClose = true;
		setTimeout(() => (forceClose = false), 150);
	}

	// Close when focus leaves the whole control (e.g. tabbing past it); Escape too.
	function onFocusOut(e: FocusEvent) {
		if (rootEl && !rootEl.contains(e.relatedTarget as Node | null)) {
			isOpen = false;
		}
	}
	function onKeydown(e: KeyboardEvent) {
		if (e.key === 'Escape') isOpen = false;
	}

	// When the dropdown opens (via focus/click), move the selector to the first
	// option so keyboard users start there.
	$effect(() => {
		if (isOpen && rootEl) {
			const root = rootEl;
			requestAnimationFrame(() => {
				root.querySelector<HTMLButtonElement>('.select-option')?.focus();
			});
		}
	});
</script>

<!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions a11y_mouse_events_have_key_events -->
<div
	class="notebook-select-root"
	class:force-close={forceClose}
	class:is-open={isOpen}
	bind:this={rootEl}
	onfocusout={onFocusOut}
	onkeydown={onKeydown}
>
	<button class="notebook-select-btn" onclick={() => (isOpen = true)} onfocus={() => (isOpen = true)} type="button">
		{value || 'Uncategorized'}
		<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
			<polyline points="6 9 12 15 18 9"></polyline>
		</svg>
	</button>

	<div class="select-dropdown">
		<button class="select-option" class:selected={!value} onclick={() => selectNb('')} type="button">Uncategorized</button>
		{#each notebooks as nb}
			<button class="select-option" class:selected={value === nb} onclick={() => selectNb(nb)} type="button">{nb}</button>
		{/each}
	</div>
</div>

<style>
	.notebook-select-root {
		position: relative;
		display: inline-block;
	}
	.notebook-select-btn {
		background: transparent;
		border: 1px solid var(--border-default, #333);
		color: var(--text-primary);
		font-family: inherit;
		font-size: 0.95rem;
		font-weight: 500;
		padding: 4px 10px;
		border-radius: var(--radius-sm, 6px);
		cursor: pointer;
		display: flex;
		align-items: center;
		gap: 8px;
		transition: background 0.1s, border-color 0.1s;
		outline: none;
	}
	.notebook-select-btn:hover,
	.notebook-select-btn:focus-visible {
		background: var(--hover-overlay, rgba(255, 255, 255, 0.05));
		border-color: var(--text-secondary, #888);
		outline: none;
	}
	.notebook-select-btn svg {
		color: var(--text-secondary);
	}
	.select-dropdown {
		display: none;
		position: absolute;
		top: calc(100% + 4px);
		left: 0;
		min-width: 160px;
		background: var(--bg-surface, #1e1e1e);
		border: 1px solid var(--border-default, #333);
		border-radius: var(--radius-sm, 6px);
		box-shadow: 0 4px 16px rgba(0, 0, 0, 0.4);
		z-index: 10000;
		flex-direction: column;
		padding: 4px;
		max-height: 200px;
		overflow-y: auto;
	}
	.notebook-select-root:not(.force-close):hover .select-dropdown,
	.notebook-select-root.is-open:not(.force-close) .select-dropdown {
		display: flex;
	}
	.select-option {
		background: transparent;
		border: none;
		color: var(--text-secondary);
		padding: 8px 12px;
		text-align: left;
		font-size: 0.95rem;
		cursor: pointer;
		border-radius: 4px;
		transition: background 0.1s, color 0.1s;
	}
	.select-option:hover {
		background: var(--hover-overlay, rgba(255, 255, 255, 0.05));
		color: var(--text-primary);
	}
	.select-option.selected {
		color: var(--accent-200, #a8c7fa);
		background: var(--hover-overlay, rgba(255, 255, 255, 0.05));
	}
</style>
