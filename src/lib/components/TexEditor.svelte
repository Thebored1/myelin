<script lang="ts">
	interface Props {
		value: string;
		onInput: (val: string) => void;
	}
	let { value, onInput }: Props = $props();

	function handleInput(e: Event) {
		const target = e.target as HTMLTextAreaElement;
		onInput(target.value);
	}
</script>

<div style="width: 100%; height: 100%; display: flex; flex-direction: column;">
	<div style="padding: 8px; background: var(--bg-panel); border-bottom: 1px solid var(--border-default); display: flex; justify-content: space-between; align-items: center;">
		<span style="font-size: 0.8rem; color: var(--text-secondary); font-family: var(--font-mono);">LaTeX Editor</span>
	</div>
	<textarea
		class="tex-textarea"
		value={value}
		oninput={handleInput}
		placeholder="Type your LaTeX code here..."
		spellcheck="false"
	></textarea>
</div>

<style>
	.tex-textarea {
		flex: 1;
		width: 100%;
		border: none;
		resize: none;
		padding: 1rem;
		background: var(--bg-page);
		color: var(--text-primary);
		font-family: var(--font-mono);
		font-size: 0.9rem;
		line-height: 1.5;
		outline: none;
	}
</style>
