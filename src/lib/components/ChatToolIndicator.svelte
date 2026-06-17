<script lang="ts">
	let { tool }: { tool: { name: string; details: string } } = $props();
	let expanded = $state(false);
</script>

<div class="chat-tool-indicator">
	<button class="tool-header" onclick={() => expanded = !expanded}>
		<svg class="dropdown-arrow" class:expanded width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
			<polyline points="9 18 15 12 9 6"></polyline>
		</svg>
		<span class="tool-name">{tool.name}</span>
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
