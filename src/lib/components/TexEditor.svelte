<script lang="ts">
	import { onMount, onDestroy } from 'svelte';
	import { EditorView, keymap, lineNumbers, highlightActiveLine, highlightActiveLineGutter, drawSelection } from '@codemirror/view';
	import { EditorState, Compartment } from '@codemirror/state';
	import { defaultKeymap, history, historyKeymap, indentWithTab } from '@codemirror/commands';
	import { StreamLanguage, syntaxHighlighting, defaultHighlightStyle, bracketMatching } from '@codemirror/language';
	import { stex } from '@codemirror/legacy-modes/mode/stex';
	import { lintGutter, setDiagnostics, type Diagnostic as CmDiagnostic } from '@codemirror/lint';
	import { theme } from '$lib/theme';

	export interface TexDiagnostic {
		line: number;
		message: string;
		severity?: 'error' | 'warning';
	}

	interface Props {
		value: string;
		onInput: (val: string) => void;
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
		diagnostics = [],
		onCompile,
		autoCompile = false,
		onToggleAuto,
		busy = false,
		statusMsg = null
	}: Props = $props();

	let host: HTMLDivElement;
	let view: EditorView | undefined;
	const themeCompartment = new Compartment();

	// One theme that defers all colors to the app's CSS vars, so it tracks the
	// light/dark toggle for free; we only flip CodeMirror's internal `dark` flag
	// (affects default selection styling) via the compartment.
	const appTheme = EditorView.theme({
		'&': { height: '100%', backgroundColor: 'var(--bg-page)', color: 'var(--text-primary)' },
		'.cm-content': { fontFamily: 'var(--font-mono)', fontSize: '0.9rem', padding: '1rem 0' },
		'.cm-scroller': { overflow: 'auto', lineHeight: '1.5' },
		'.cm-gutters': { backgroundColor: 'var(--bg-panel)', color: 'var(--text-secondary)', border: 'none' },
		'.cm-activeLine': { backgroundColor: 'color-mix(in srgb, var(--accent-200, #6ea8fe) 9%, transparent)' },
		'.cm-activeLineGutter': { backgroundColor: 'transparent', color: 'var(--text-primary)' },
		'.cm-cursor': { borderLeftColor: 'var(--text-primary)' },
		'&.cm-focused': { outline: 'none' },
		'.cm-lint-marker': { width: '0.8em', height: '0.8em' }
	});

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

	onMount(() => {
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
					lintGutter(),
					StreamLanguage.define(stex),
					syntaxHighlighting(defaultHighlightStyle, { fallback: true }),
					keymap.of([indentWithTab, ...defaultKeymap, ...historyKeymap]),
					themeCompartment.of(makeThemeExt($theme === 'light')),
					EditorView.updateListener.of((u) => {
						if (u.docChanged) onInput(u.state.doc.toString());
					})
				]
			})
		});
		// Apply any diagnostics that arrived before the view existed.
		if (diagnostics.length) view.dispatch(setDiagnostics(view.state, toCmDiagnostics(view.state)));
	});

	onDestroy(() => view?.destroy());

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
	<div class="tex-toolbar">
		<div class="tex-tools">
			<button class="btn-ghost tex-btn" style="font-weight: bold;" onclick={() => insertText('\\textbf{', '}')} title="Bold">B</button>
			<button class="btn-ghost tex-btn" style="font-style: italic;" onclick={() => insertText('\\textit{', '}')} title="Italic">I</button>
			<button class="btn-ghost tex-btn" onclick={() => insertText('\\section{', '}')} title="Section">§</button>
			<button class="btn-ghost tex-btn" style="font-family: serif;" onclick={() => insertText('\\begin{equation}\n', '\n\\end{equation}')} title="Equation">∑</button>
			<button class="btn-ghost tex-btn" onclick={() => insertText('\\begin{itemize}\n\\item ', '\n\\end{itemize}')} title="Itemize">•=</button>
		</div>
		{#if onCompile}
			<div class="tex-compile">
				{#if statusMsg}<span class="tex-status">{statusMsg}</span>{/if}
				<button
					class="btn-ghost tex-auto"
					class:on={autoCompile}
					title="Recompile automatically a couple of seconds after you stop typing"
					onclick={onToggleAuto}
				>Auto: {autoCompile ? 'on' : 'off'}</button>
				<button class="primary" disabled={busy} onclick={onCompile}>Compile to PDF</button>
			</div>
		{/if}
	</div>
	<div bind:this={host} class="cm-host"></div>
</div>

<style>
	.tex-toolbar {
		padding: 6px 8px;
		background: var(--bg-panel);
		border-bottom: 1px solid var(--border-default);
		display: flex;
		justify-content: space-between;
		align-items: center;
		gap: 8px;
		row-gap: 6px;
		flex-wrap: wrap;
	}
	/* Keep the two groups intact and let them wrap to a new row when the pane is
	   narrow, rather than shrinking and overlapping each other. */
	.tex-tools,
	.tex-compile {
		display: flex;
		gap: 6px;
		align-items: center;
		flex-shrink: 0;
	}
	.tex-tools {
		gap: 4px;
	}
	.tex-btn {
		padding: 4px 8px;
		font-size: 0.8rem;
		flex-shrink: 0;
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
