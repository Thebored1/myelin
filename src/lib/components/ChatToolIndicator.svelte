<script lang="ts">
	let { tool }: { tool: { name: string; details: string } } = $props();
	let expanded = $state(false);

	// Pick a distinct icon per tool kind so web fetches (and the others) are
	// recognizable at a glance, not just identical rows of text.
	let kind = $derived.by(() => {
		const n = tool.name.toLowerCase();
		if (n.includes('web') || n.includes('fetch') || n.includes('url')) return 'web';
		if (n.includes('search')) return 'search';
		if (n.includes('read')) return 'read';
		if (
			n.includes('write') || n.includes('append') || n.includes('note') ||
			n.includes('text') || n.includes('clear') || n.includes('replace') || n.includes('delete')
		)
			return 'edit';
		return 'tool';
	});

	// A concise one-line preview of what the tool is acting on — the URL it's
	// fetching, the search query, the note title — shown inline so you can see
	// what it's doing (and which page it's on) without expanding.
	let preview = $derived.by(() => {
		const d = (tool.details || '').replace(/\s+/g, ' ').trim();
		if (!d) return '';
		// For a URL, drop the scheme so the domain/path reads cleanly.
		const cleaned = d.replace(/^https?:\/\//i, '');
		return cleaned.length > 64 ? cleaned.slice(0, 63) + '…' : cleaned;
	});
</script>

<div class="chat-tool-indicator">
	<button class="tool-header" onclick={() => expanded = !expanded}>
		<svg class="dropdown-arrow" class:expanded width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
			<polyline points="9 18 15 12 9 6"></polyline>
		</svg>
		<svg class="tool-icon" width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
			{#if kind === 'web'}
				<circle cx="12" cy="12" r="9"></circle>
				<path d="M3 12h18"></path>
				<path d="M12 3a14 14 0 0 1 0 18 14 14 0 0 1 0-18"></path>
			{:else if kind === 'search'}
				<circle cx="11" cy="11" r="7"></circle>
				<path d="M21 21l-4.3-4.3"></path>
			{:else if kind === 'read'}
				<path d="M2 6s3-2 5-2 5 2 5 2v14s-3-2-5-2-5 2-5 2z"></path>
				<path d="M12 6s3-2 5-2 5 2 5 2v14s-3-2-5-2-5 2-5 2z"></path>
			{:else if kind === 'edit'}
				<path d="M12 20h9"></path>
				<path d="M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4z"></path>
			{:else}
				<path d="M14.7 6.3a4 4 0 0 1-5.4 5.4L4 17l3 3 5.3-5.3a4 4 0 0 1 5.4-5.4l-2.6 2.6-2-2z"></path>
			{/if}
		</svg>
		<span class="tool-name" class:web={kind === 'web'}>{tool.name}</span>
		{#if preview}
			<span class="tool-detail-inline" title={tool.details}>· {preview}</span>
		{/if}
	</button>
	{#if expanded && tool.details}
		<div class="indicator-content">
			<pre class="tool-details-text">{tool.details}</pre>
		</div>
	{/if}
</div>

<style>
	.chat-tool-indicator {
		display: flex;
		flex-direction: column;
		width: 100%;
		margin-top: var(--space-1);
		margin-bottom: var(--space-1);
	}
	
	.tool-header {
		display: flex;
		align-items: center;
		gap: 6px;
		background: none;
		border: none;
		padding: 4px 0;
		font-size: 0.8rem;
		font-weight: 500;
		color: var(--text-secondary);
		cursor: pointer;
		text-align: left;
		opacity: 0.8;
		transition: opacity 0.2s, color 0.2s;
	}

	.tool-header:hover {
		opacity: 1;
		color: var(--text-primary);
	}

	.dropdown-arrow {
		transition: transform 0.2s ease;
	}

	.dropdown-arrow.expanded {
		transform: rotate(90deg);
	}

	.tool-icon {
		flex-shrink: 0;
		opacity: 0.85;
	}

	/* Web fetches stand out a touch from note tools. */
	.tool-name.web {
		color: var(--text-primary);
	}

	/* Inline preview of the URL / query / target — what it's doing, at a glance. */
	.tool-detail-inline {
		font-size: 0.75rem;
		color: var(--text-muted);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		min-width: 0;
		flex: 1;
	}

	.indicator-content {
		padding-left: 18px; /* Align with text, leaving space for the arrow */
		padding-top: var(--space-1);
		padding-bottom: var(--space-2);
	}

	.tool-details-text {
		font-family: var(--font-mono);
		font-size: 0.75rem;
		color: var(--text-muted);
		margin: 0;
		white-space: pre-wrap;
		overflow-wrap: anywhere;
	}
</style>
