<script lang="ts">
	let {
		showToolbar,
		toolMode,
		toolbarStyle,
		applyHighlight,
		handleCopy,
		handleQuote
	}: {
		showToolbar: boolean;
		toolMode: string;
		toolbarStyle: string;
		applyHighlight: (color: string) => void;
		handleCopy: () => void;
		handleQuote: () => void;
	} = $props();
</script>

{#if showToolbar && toolMode === 'select'}
			<div class="floating-toolbar" style={toolbarStyle}>
				<div class="swatch-group">
					<button
						class="swatch-btn"
						aria-label="Highlight Yellow"
						style="--swatch-color: rgba(253, 224, 71, 0.4); --swatch-border: #eab308;"
						onclick={() => applyHighlight('rgba(253, 224, 71, 0.4)')}
						><span class="swatch-icon">T</span></button
					>
					<button
						class="swatch-btn"
						aria-label="Highlight Green"
						style="--swatch-color: rgba(134, 239, 172, 0.4); --swatch-border: #22c55e;"
						onclick={() => applyHighlight('rgba(134, 239, 172, 0.4)')}
						><span class="swatch-icon">T</span></button
					>
					<button
						class="swatch-btn"
						aria-label="Highlight Blue"
						style="--swatch-color: rgba(147, 197, 253, 0.4); --swatch-border: #3b82f6;"
						onclick={() => applyHighlight('rgba(147, 197, 253, 0.4)')}
						><span class="swatch-icon">T</span></button
					>
					<button
						class="swatch-btn"
						aria-label="Highlight Pink"
						style="--swatch-color: rgba(249, 168, 212, 0.4); --swatch-border: #ec4899;"
						onclick={() => applyHighlight('rgba(249, 168, 212, 0.4)')}
						><span class="swatch-icon">T</span></button
					>
					<button
						class="swatch-btn"
						aria-label="Highlight Purple"
						style="--swatch-color: rgba(196, 181, 253, 0.4); --swatch-border: #a855f7;"
						onclick={() => applyHighlight('rgba(196, 181, 253, 0.4)')}
						><span class="swatch-icon">T</span></button
					>
					<button
						class="swatch-btn"
						aria-label="Highlight Orange"
						style="--swatch-color: rgba(253, 186, 116, 0.4); --swatch-border: #f97316;"
						onclick={() => applyHighlight('rgba(253, 186, 116, 0.4)')}
						><span class="swatch-icon">T</span></button
					>
				</div>
				<div class="divider"></div>
				<button class="action-btn" onclick={handleCopy} aria-label="Copy Text" title="Copy">
					<svg
						viewBox="0 0 24 24"
						width="14"
						height="14"
						stroke="currentColor"
						stroke-width="2"
						fill="none"
						><rect x="9" y="9" width="13" height="13" rx="2" ry="2"></rect><path
							d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"
						></path></svg
					>
				</button>
				<button
					class="action-btn"
					onclick={handleQuote}
					aria-label="Quote to Note"
					title="Quote to Note"
				>
					<svg
						viewBox="0 0 24 24"
						width="14"
						height="14"
						stroke="currentColor"
						stroke-width="2"
						fill="none"
					>
						<path d="M11 4H4a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7"></path>
						<path d="M18.5 2.5a2.121 2.121 0 0 1 3 3L12 15l-4 1 1-4 9.5-9.5z"></path>
					</svg>
				</button>
			</div>
		{/if}