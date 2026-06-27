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
	}
	let { value, onInput, diagnostics = [] }: Props = $props();

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
					EditorView.lineWrapping,
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
	<div bind:this={host} class="cm-host"></div>
</div>

<style>
	.cm-host {
		flex: 1;
		min-height: 0;
		overflow: hidden;
	}
	.cm-host :global(.cm-editor) {
		height: 100%;
	}
</style>
