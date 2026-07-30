<script lang="ts">
	import { onMount, onDestroy } from 'svelte';
	import {
		EditorView,
		keymap,
		lineNumbers,
		highlightActiveLine,
		highlightActiveLineGutter,
		drawSelection
	} from '@codemirror/view';
	import { EditorState, Compartment } from '@codemirror/state';
	import {
		defaultKeymap,
		history,
		historyKeymap,
		indentWithTab,
		undo,
		redo,
		undoDepth,
		redoDepth
	} from '@codemirror/commands';
	import {
		StreamLanguage,
		syntaxHighlighting,
		HighlightStyle,
		bracketMatching
	} from '@codemirror/language';
	import { tags } from '@lezer/highlight';
	import { stex } from '@codemirror/legacy-modes/mode/stex';
	import { lintGutter, setDiagnostics, type Diagnostic as CmDiagnostic } from '@codemirror/lint';
	import { openSearchPanel, search } from '@codemirror/search';
	import { theme } from '$lib/theme';
	// Vditor uses this shared material SVG sprite for the Markdown toolbar.
	// Load it here too so a LaTeX-only note gets the exact same icon artwork.
	import 'vditor/dist/js/icons/material.js';

	export interface TexDiagnostic {
		line: number;
		message: string;
		severity?: 'error' | 'warning';
	}

	interface Props {
		value: string;
		onInput: (val: string) => void;
		onAiTargetChange?: (target: {
			text: string;
			before: string;
			after: string;
			cursor: boolean;
		}) => void;
		onPickImage?: () => Promise<string | null>;
		diagnostics?: TexDiagnostic[];
		// Compile controls, rendered into the same toolbar as the format buttons.
		onCompile?: () => void;
		autoCompile?: boolean;
		onToggleAuto?: () => void;
		busy?: boolean;
		statusMsg?: string | null;
	}
	let {
		value,
		onInput,
		onAiTargetChange,
		onPickImage,
		diagnostics = [],
		onCompile,
		autoCompile = false,
		onToggleAuto,
		busy = false,
		statusMsg = null
	}: Props = $props();

	let host: HTMLDivElement;
	let headingMenu: HTMLDetailsElement;
	let view: EditorView | undefined;
	let canUndo = $state(false);
	let canRedo = $state(false);
	const themeCompartment = new Compartment();

	function emitAiTarget(editor: EditorView) {
		if (!onAiTargetChange || !editor.hasFocus) return;
		const { from, to } = editor.state.selection.main;
		const source = editor.state.doc.toString();
		const N = 80;
		onAiTargetChange({
			text: source.slice(from, to),
			before: source.slice(Math.max(0, from - N), from),
			after: source.slice(to, Math.min(source.length, to + N)),
			cursor: from === to
		});
	}

	// One theme that defers all colors to the app's CSS vars, so it tracks the
	// light/dark toggle for free; we only flip CodeMirror's internal `dark` flag
	// (affects default selection styling) via the compartment.
	const appTheme = EditorView.theme({
		'&': { height: '100%', backgroundColor: 'var(--bg-page)', color: 'var(--text-primary)' },
		'.cm-content': { fontFamily: 'var(--font-mono)', fontSize: '0.9rem', padding: '1rem 0' },
		'.cm-scroller': { overflow: 'auto', lineHeight: '1.5' },
		'.cm-gutters': {
			backgroundColor: 'var(--bg-panel)',
			color: 'var(--text-secondary)',
			border: 'none'
		},
		'.cm-activeLine': {
			backgroundColor: 'color-mix(in srgb, var(--accent-200, #6ea8fe) 9%, transparent)'
		},
		'.cm-activeLineGutter': { backgroundColor: 'transparent', color: 'var(--text-primary)' },
		'.cm-cursor': { borderLeftColor: 'var(--text-primary)' },
		'&.cm-focused > .cm-scroller > .cm-selectionLayer .cm-selectionBackground, .cm-selectionBackground':
			{ backgroundColor: 'var(--bg-selection) !important' },
		'.cm-content ::selection': {
			backgroundColor: 'var(--bg-selection) !important',
			color: 'var(--text-inverse) !important'
		},
		'&.cm-focused': { outline: 'none' },
		'.cm-lint-marker': { width: '0.8em', height: '0.8em' }
	});

	const latexHighlightStyle = HighlightStyle.define([
		{ tag: [tags.keyword, tags.processingInstruction], color: 'var(--accent-200)' },
		{ tag: [tags.tagName, tags.typeName, tags.className], color: '#4f7fd5' },
		{ tag: [tags.string, tags.special(tags.string)], color: 'var(--success)' },
		{ tag: [tags.number, tags.bool, tags.atom], color: '#9c6ade' },
		{ tag: [tags.comment, tags.docComment], color: 'var(--text-secondary)', fontStyle: 'italic' },
		{ tag: [tags.variableName, tags.propertyName, tags.attributeName], color: '#16856b' },
		{ tag: [tags.bracket, tags.paren, tags.punctuation], color: 'var(--neutral-500)' },
		{ tag: [tags.strong], fontWeight: '700' },
		{ tag: [tags.emphasis], fontStyle: 'italic' },
		{ tag: [tags.invalid], color: 'var(--danger-text)', textDecoration: 'underline wavy' }
	]);

	function makeThemeExt(isLight: boolean) {
		return [appTheme, EditorView.theme({}, { dark: !isLight })];
	}

	function toCmDiagnostics(state: EditorState): CmDiagnostic[] {
		const lineCount = state.doc.lines;
		return diagnostics
			.filter((d) => d.line >= 1 && d.line <= lineCount)
			.map((d) => {
				const line = state.doc.line(Math.min(d.line, lineCount));
				return {
					from: line.from,
					to: line.to,
					severity: d.severity ?? 'error',
					message: d.message
				} satisfies CmDiagnostic;
			});
	}

	export function insertText(before: string, after: string = '') {
		if (!view) return;
		const { from, to } = view.state.selection.main;
		const selected = view.state.sliceDoc(from, to);
		view.dispatch({
			changes: { from, to, insert: before + selected + after },
			selection: { anchor: from + before.length, head: from + before.length + selected.length }
		});
		view.focus();
	}

	export function focusEditor() {
		view?.focus();
	}

	function runEdit(command: (view: EditorView) => boolean) {
		if (view) {
			command(view);
			view.focus();
		}
	}

	function closeHeadingMenu(event: PointerEvent | KeyboardEvent) {
		if (!headingMenu?.open) return;
		if (event instanceof KeyboardEvent) {
			if (event.key === 'Escape') headingMenu.removeAttribute('open');
			return;
		}
		if (!headingMenu.contains(event.target as Node)) headingMenu.removeAttribute('open');
	}

	const headingKeymap = [
		['Alt-Ctrl-1', 'section'],
		['Alt-Ctrl-2', 'subsection'],
		['Alt-Ctrl-3', 'subsubsection'],
		['Alt-Ctrl-4', 'paragraph'],
		['Alt-Ctrl-5', 'subparagraph'],
		['Alt-Ctrl-6', 'chapter']
	].map(([key, level]) => ({
		key,
		run: () => {
			insertPlaceholder(`\\${level}{`, 'Heading', '}');
			return true;
		}
	}));

	function insertPlaceholder(before: string, placeholder: string, after = '') {
		if (!view) return;
		const { from, to } = view.state.selection.main;
		const selected = view.state.sliceDoc(from, to) || placeholder;
		view.dispatch({
			changes: { from, to, insert: before + selected + after },
			selection: view.state.sliceDoc(from, to)
				? { anchor: from + before.length + selected.length }
				: { anchor: from + before.length, head: from + before.length + placeholder.length }
		});
		view.focus();
	}

	function toggleComment() {
		if (!view) return;
		const { from, to } = view.state.selection.main;
		const first = view.state.doc.lineAt(from);
		const last = view.state.doc.lineAt(to);
		const lines = [];
		for (let n = first.number; n <= last.number; n++) lines.push(view.state.doc.line(n));
		const uncomment = lines.every((line) => line.text.trimStart().startsWith('%'));
		const changes = lines.map((line) => {
			if (!uncomment) return { from: line.from, insert: '% ' };
			const offset = line.text.indexOf('%');
			const remove = line.text.slice(offset).startsWith('% ') ? 2 : 1;
			return { from: line.from + offset, to: line.from + offset + remove, insert: '' };
		});
		view.dispatch({ changes });
		view.focus();
	}

	function formatLatex() {
		if (!view) return;
		let depth = 0;
		const formatted = view.state.doc.toString().split('\n').map((raw) => {
			const line = raw.trimEnd();
			const text = line.trimStart();
			if (/^\\end\{/.test(text)) depth = Math.max(0, depth - 1);
			const result = text ? `${'  '.repeat(depth)}${text}` : '';
			if (/^\\begin\{/.test(text) && !/^\\begin\{document\}/.test(text)) depth += 1;
			return result;
		}).join('\n');
		view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: formatted } });
		view.focus();
	}

	async function pickImage() {
		const path = await onPickImage?.();
		if (path) insertPlaceholder('\\includegraphics[width=\\linewidth]{', path, '}');
	}

	onMount(() => {
		document.addEventListener('pointerdown', closeHeadingMenu);
		document.addEventListener('keydown', closeHeadingMenu);
		view = new EditorView({
			parent: host,
			state: EditorState.create({
				doc: value,
				extensions: [
					lineNumbers(),
					highlightActiveLine(),
					highlightActiveLineGutter(),
					drawSelection(),
					history(),
					bracketMatching(),
					search(),
					lintGutter(),
					StreamLanguage.define(stex),
					syntaxHighlighting(latexHighlightStyle, { fallback: true }),
					keymap.of([...headingKeymap, indentWithTab, ...defaultKeymap, ...historyKeymap]),
					themeCompartment.of(makeThemeExt($theme === 'light')),
					EditorView.updateListener.of((u) => {
						if (u.docChanged) onInput(u.state.doc.toString());
						canUndo = undoDepth(u.state) > 0;
						canRedo = redoDepth(u.state) > 0;
						if (u.selectionSet || u.focusChanged || u.docChanged) emitAiTarget(u.view);
					})
				]
			})
		});
		// Apply any diagnostics that arrived before the view existed.
		if (diagnostics.length) view.dispatch(setDiagnostics(view.state, toCmDiagnostics(view.state)));
	});

	onDestroy(() => {
		document.removeEventListener('pointerdown', closeHeadingMenu);
		document.removeEventListener('keydown', closeHeadingMenu);
		view?.destroy();
	});

	// External content changes (e.g. the AI writes to the note) — push into the
	// editor only when it actually differs, so we don't fight the user's typing.
	$effect(() => {
		const incoming = value;
		if (view && incoming !== view.state.doc.toString()) {
			view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: incoming } });
		}
	});

	// Keep CodeMirror's dark flag in sync with the app theme.
	$effect(() => {
		const isLight = $theme === 'light';
		if (view) view.dispatch({ effects: themeCompartment.reconfigure(makeThemeExt(isLight)) });
	});

	// Re-apply diagnostics whenever the parent updates them (compile results).
	$effect(() => {
		void diagnostics;
		if (view) view.dispatch(setDiagnostics(view.state, toCmDiagnostics(view.state)));
	});
</script>

<div style="width: 100%; height: 100%; min-width: 0; display: flex; flex-direction: column;">
	<div class="tex-toolbar" class:dark={$theme !== 'light'}>
		<div class="tex-tools">
			<button class="tex-btn" onclick={() => insertText('\\textbf{', '}')} aria-label="Bold" title="Bold"><svg><use href="#vditor-icon-bold"></use></svg></button>
			<button class="tex-btn" onclick={() => insertText('\\textit{', '}')} aria-label="Italic" title="Italic"><svg><use href="#vditor-icon-italic"></use></svg></button>
			<details class="tex-heading-menu" bind:this={headingMenu}>
				<summary class="tex-btn" aria-label="Heading level" title="Heading level"><svg><use href="#vditor-icon-headings"></use></svg></summary>
				<div class="tex-heading-popover">
					<button onclick={(e) => { insertPlaceholder('\\section{', 'Heading', '}'); e.currentTarget.closest('details')?.removeAttribute('open'); }}>Section <span>&lt;Alt+Ctrl+1&gt;</span></button>
					<button onclick={(e) => { insertPlaceholder('\\subsection{', 'Heading', '}'); e.currentTarget.closest('details')?.removeAttribute('open'); }}>Subsection <span>&lt;Alt+Ctrl+2&gt;</span></button>
					<button onclick={(e) => { insertPlaceholder('\\subsubsection{', 'Heading', '}'); e.currentTarget.closest('details')?.removeAttribute('open'); }}>Subsubsection <span>&lt;Alt+Ctrl+3&gt;</span></button>
					<button onclick={(e) => { insertPlaceholder('\\paragraph{', 'Heading', '}'); e.currentTarget.closest('details')?.removeAttribute('open'); }}>Paragraph <span>&lt;Alt+Ctrl+4&gt;</span></button>
					<button onclick={(e) => { insertPlaceholder('\\subparagraph{', 'Heading', '}'); e.currentTarget.closest('details')?.removeAttribute('open'); }}>Subparagraph <span>&lt;Alt+Ctrl+5&gt;</span></button>
					<button onclick={(e) => { insertPlaceholder('\\chapter{', 'Heading', '}'); e.currentTarget.closest('details')?.removeAttribute('open'); }}>Chapter <span>&lt;Alt+Ctrl+6&gt;</span></button>
				</div>
			</details>
			<span class="tex-divider"></span>
			<button class="tex-btn" onclick={() => insertText('$', '$')} aria-label="Inline math" title="Inline math"><svg><use href="#vditor-icon-inline-code"></use></svg></button>
			<button class="tex-btn" onclick={() => insertText('\\begin{equation}\n', '\n\\end{equation}')} aria-label="Math block" title="Math block"><svg><use href="#vditor-icon-inline-code"></use></svg></button>
			<button class="tex-btn" onclick={() => insertText('\\begin{itemize}\n\\item ', '\n\\end{itemize}')} aria-label="Bulleted list" title="Bulleted list"><svg><use href="#vditor-icon-list"></use></svg></button>
			<button class="tex-btn" onclick={() => insertText('\\begin{enumerate}\n\\item ', '\n\\end{enumerate}')} aria-label="Numbered list" title="Numbered list"><svg><use href="#vditor-icon-ordered-list"></use></svg></button>
			<button class="tex-btn" onclick={() => insertText('\\begin{quote}\n', '\n\\end{quote}')} aria-label="Quote" title="Quote"><svg><use href="#vditor-icon-quote"></use></svg></button>
			<button class="tex-btn" onclick={() => insertText('\\begin{verbatim}\n', '\n\\end{verbatim}')} aria-label="Code block" title="Code block"><svg><use href="#vditor-icon-code"></use></svg></button>
			<span class="tex-divider"></span>
			<button class="tex-btn" onclick={() => insertPlaceholder('\\href{url}{', 'text', '}')} aria-label="Link" title="Link"><svg><use href="#vditor-icon-link"></use></svg></button>
			<button class="tex-btn" onclick={() => void pickImage()} aria-label="Insert image" title="Insert image"><svg><use href="#vditor-icon-upload"></use></svg></button>
			<button class="tex-btn" onclick={() => insertText('\\begin{table}[ht]\n\\centering\n\\begin{tabular}{ll}\nA & B \\\\\n\\hline\n1 & 2 \\\\\n\\end{tabular}\n\\caption{Caption}\n\\end{table}')} aria-label="Table" title="Table"><svg><use href="#vditor-icon-table"></use></svg></button>
			<button class="tex-btn" onclick={() => insertPlaceholder('\\cite{', 'key', '}')} aria-label="Citation" title="Citation"><svg><use href="#vditor-icon-link"></use></svg></button>
			<button class="tex-btn" onclick={() => insertPlaceholder('\\label{', 'label', '}')} aria-label="Label" title="Label"><svg><use href="#vditor-icon-check"></use></svg></button>
			<button class="tex-btn" onclick={() => insertPlaceholder('\\ref{', 'label', '}')} aria-label="Reference" title="Reference"><svg><use href="#vditor-icon-link"></use></svg></button>
			<button class="tex-btn" onclick={toggleComment} aria-label="Comment or uncomment" title="Comment or uncomment"><svg><use href="#vditor-icon-comment"></use></svg></button>
			<button class="tex-btn" onclick={formatLatex} aria-label="Format document" title="Format document"><svg><use href="#vditor-icon-indent"></use></svg></button>
			<button class="tex-btn" onclick={() => runEdit(openSearchPanel)} aria-label="Find and replace" title="Find and replace"><svg><use href="#vditor-icon-preview"></use></svg></button>
			<span class="tex-divider"></span>
			<button class="tex-btn" disabled={!canUndo} onclick={() => runEdit(undo)} aria-label="Undo" title="Undo"><svg><use href="#vditor-icon-undo"></use></svg></button>
			<button class="tex-btn" disabled={!canRedo} onclick={() => runEdit(redo)} aria-label="Redo" title="Redo"><svg><use href="#vditor-icon-redo"></use></svg></button>
		</div>
		{#if onCompile}
			<div class="tex-compile">
				{#if statusMsg}<span class="tex-status">{statusMsg}</span>{/if}
				<button
					class="tex-auto"
					class:on={autoCompile}
					title="Recompile automatically a couple of seconds after you stop typing"
					onclick={onToggleAuto}>Auto: {autoCompile ? 'on' : 'off'}</button
				>
				<button class="primary" disabled={busy} onclick={onCompile}>Compile to PDF</button>
			</div>
		{/if}
	</div>
	<div bind:this={host} class="cm-host"></div>
</div>

<style>
	.tex-toolbar {
		--toolbar-icon-color: #586069;
		--toolbar-icon-hover-color: #4285f4;
		display: flex;
		justify-content: space-between;
		align-items: center;
		gap: 0.4rem;
		padding: var(--space-2) var(--space-4);
		min-height: 49px;
		height: 49px;
		box-sizing: border-box;
		background: var(--bg-panel-blur);
		border-bottom: 1px solid var(--border-subtle);
		font-family: var(--font-mono);
		font-size: 0.85rem;
		z-index: 100;
		overflow: visible;
		scrollbar-width: none;
	}
	.tex-toolbar.dark {
		--toolbar-icon-color: #b9b9b9;
		--toolbar-icon-hover-color: #fff;
	}
	.tex-toolbar::-webkit-scrollbar {
		display: none;
	}
	.tex-tools,
	.tex-compile {
		display: flex;
		gap: 0.25rem;
		align-items: center;
		flex-shrink: 0;
	}
	.tex-divider {
		width: 1px;
		height: 20px;
		margin: 0 0.25rem;
		background: var(--border-default);
		flex: 0 0 auto;
	}
	.tex-heading-menu {
		position: relative;
		height: 35px;
	}
	.tex-heading-menu summary {
		list-style: none;
	}
	.tex-heading-menu summary::-webkit-details-marker {
		display: none;
	}
	.tex-heading-popover {
		position: absolute;
		top: 39px;
		left: 0;
		z-index: 500;
		width: max-content;
		min-width: 80px;
		max-width: 250px;
		padding: 5px 0;
		border: 0;
		border-radius: 3px;
		background: var(--bg-page);
		box-shadow: 0 1px 2px var(--shadow-color);
		line-height: 20px;
	}
	.tex-heading-popover::before {
		position: absolute;
		top: -14px;
		left: 5px;
		width: 0;
		height: 0;
		border: 7px solid transparent;
		border-bottom-color: var(--bg-page);
		content: '';
	}
	.tex-heading-popover button {
		display: block;
		width: 100%;
		padding: 3px 10px;
		border: 0;
		border-radius: 0;
		background-color: transparent;
		color: var(--toolbar-icon-color);
		font-family: var(--font-mono);
		font-size: 12px;
		line-height: 20px;
		text-align: left;
		white-space: nowrap;
		cursor: pointer;
	}
	.tex-heading-popover button:hover {
		background-color: var(--bg-panel-blur);
		color: var(--toolbar-icon-hover-color);
	}
	.tex-btn,
	.tex-auto {
		background: transparent;
		border: 0;
		color: var(--toolbar-icon-color);
		padding: 10px 5px;
		border-radius: 0;
		cursor: pointer;
		display: flex;
		align-items: center;
		justify-content: center;
		flex-shrink: 0;
		height: 35px;
		width: 25px;
		min-width: 25px;
		box-sizing: border-box;
	}
	.tex-btn:hover,
	.tex-auto:hover {
		color: var(--toolbar-icon-hover-color);
		background: transparent;
	}
	.tex-btn:disabled {
		color: color-mix(in srgb, var(--toolbar-icon-color) 38%, transparent);
		cursor: not-allowed;
	}
	.tex-btn:disabled:hover {
		color: color-mix(in srgb, var(--toolbar-icon-color) 38%, transparent);
	}
	.tex-btn:focus-visible,
	.tex-auto:focus-visible {
		outline: 1px solid var(--accent-200);
		outline-offset: -1px;
	}
	.tex-btn svg {
		width: 15px;
		height: 15px;
		fill: currentColor;
		stroke: currentColor;
		stroke-width: 0;
		pointer-events: none;
	}
	.tex-status {
		font-size: 0.78rem;
		color: var(--text-secondary);
		flex-shrink: 1;
		min-width: 0;
		max-width: 16rem;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.tex-auto.on {
		color: var(--accent-200);
		border-color: var(--accent-200);
	}
	.cm-host {
		flex: 1;
		min-height: 0;
		min-width: 0; /* let the editor shrink so its scroller can scroll, not the pane clip */
		overflow: hidden;
	}
	.cm-host :global(.cm-editor) {
		height: 100%;
		width: 100%;
		max-width: 100%;
	}
	/* Long lines (no wrap) scroll horizontally within the editor. */
	.cm-host :global(.cm-scroller) {
		overflow-x: auto;
	}
</style>
