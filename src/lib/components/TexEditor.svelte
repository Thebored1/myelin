<script lang="ts">
	interface Props {
		value: string;
		onInput: (val: string) => void;
	}
	let { value, onInput }: Props = $props();

	let textarea: HTMLTextAreaElement;

	function handleInput(e: Event) {
		const target = e.target as HTMLTextAreaElement;
		onInput(target.value);
	}

	function insertText(before: string, after: string = '') {
		if (!textarea) return;
		const start = textarea.selectionStart;
		const end = textarea.selectionEnd;
		const selected = value.substring(start, end);
		const replacement = before + selected + after;
		const newValue = value.substring(0, start) + replacement + value.substring(end);
		onInput(newValue);
		
		// Restore focus and selection
		setTimeout(() => {
			textarea.focus();
			textarea.setSelectionRange(start + before.length, start + before.length + selected.length);
		}, 0);
	}
</script>

<div style="width: 100%; height: 100%; display: flex; flex-direction: column;">
	<div style="padding: 8px; background: var(--bg-panel); border-bottom: 1px solid var(--border-default); display: flex; justify-content: space-between; align-items: center;">
		<div style="display: flex; gap: 4px; align-items: center;">
			<span style="font-size: 0.8rem; color: var(--text-secondary); font-family: var(--font-mono); margin-right: 12px;">LaTeX Editor</span>
			<button class="btn-ghost" style="padding: 4px 8px; font-size: 0.8rem; font-weight: bold;" onclick={() => insertText('\\textbf{', '}')} title="Bold">B</button>
			<button class="btn-ghost" style="padding: 4px 8px; font-size: 0.8rem; font-style: italic;" onclick={() => insertText('\\textit{', '}')} title="Italic">I</button>
			<button class="btn-ghost" style="padding: 4px 8px; font-size: 0.8rem;" onclick={() => insertText('\\section{', '}')} title="Section">§</button>
			<button class="btn-ghost" style="padding: 4px 8px; font-size: 0.8rem; font-family: serif;" onclick={() => insertText('\\begin{equation}\n', '\n\\end{equation}')} title="Equation">∑</button>
			<button class="btn-ghost" style="padding: 4px 8px; font-size: 0.8rem;" onclick={() => insertText('\\begin{itemize}\n\\item ', '\n\\end{itemize}')} title="Itemize">•=</button>
		</div>
	</div>
	<textarea
		bind:this={textarea}
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
