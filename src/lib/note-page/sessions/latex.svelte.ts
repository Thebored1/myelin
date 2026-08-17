import { invoke } from '@tauri-apps/api/core';
import { open as openFileDialog } from '@tauri-apps/plugin-dialog';
import type { ControllerContext } from '$lib/controller-context';

/** LaTeX image, compile, preview, and auto-compile lifecycle. */
export function createLatexSession(rawContext: object) {
	const ctx = rawContext as ControllerContext;
	async function pickLatexImage(): Promise<string | null> {
		if (!ctx.note) return null;
		const selected = await openFileDialog({
			multiple: false,
			filters: [{ name: 'LaTeX images', extensions: ['png', 'jpg', 'jpeg', 'pdf'] }]
		});
		if (!selected || Array.isArray(selected)) return null;
		try {
			return await invoke<string>('import_latex_asset', {
				noteId: ctx.note.id,
				sourcePath: selected
			});
		} catch (error) {
			ctx.texCompileError = `Could not import image: ${String(error)}`;
			ctx.texPreviewStatus = 'error';
			return null;
		}
	}

	// Compile the open .tex note to PDF and show it in the split preview pane.
	// Shared by the manual button and the debounced auto-compile.
	async function compileTex(opts: { manual?: boolean } = {}) {
		const manual = opts.manual === true;
		if (!ctx.note) return;
		if (!manual && !ctx.texCacheWarmed) return;
		if (ctx.texCompiling) {
			ctx.texCompileQueued = true;
			ctx.texPreviewStatus = 'pending';
			return;
		}
		ctx.texCompiling = true;
		ctx.texPreviewStatus = 'compiling';
		ctx.texCompileError = null;
		if (manual) ctx.isBusy = true;
		try {
			let processedRevision = -1;
			let processedNoteId: string = ctx.note.id;
			do {
				ctx.texCompileQueued = false;
				const revision = ctx.texRevision;
				const noteId: string = ctx.note.id;
				processedNoteId = noteId;
				const source = ctx.draftBody;
				try {
					if (manual) await ctx.saveNote();
					const pdfBytes = await invoke<ArrayBuffer>('compile_latex', { noteId, source });
					// Never let a late compile replace a newer edit or another note's PDF.
					if (ctx.note?.id === noteId && revision === ctx.texRevision) {
						ctx.activeSourceBytes = new Uint8Array(pdfBytes);
						ctx.sourceMaterialType = 'pdf';
						ctx.showAttachedNote = true;
						ctx.texDiagnostics = [];
						ctx.texCompileError = null;
						ctx.texCacheWarmed = true;
						ctx.texPreviewStatus = 'current';
					}
				} catch (e) {
					// Errors from obsolete snapshots are intentionally discarded; the
					// next queued revision will report the relevant result instead.
					if (ctx.note?.id === noteId && revision === ctx.texRevision) {
						const info = parseLatexError(e);
						ctx.texDiagnostics = info.diagnostics;
						ctx.texCompileError = info.message;
						ctx.texPreviewStatus = 'error';
					}
				}
				processedRevision = revision;
			} while (
				ctx.texCompileQueued ||
				(ctx.note?.id === processedNoteId && ctx.texRevision !== processedRevision)
			);
		} finally {
			ctx.texCompiling = false;
			if (ctx.texCompileQueued) ctx.texPreviewStatus = 'pending';
			if (manual) ctx.isBusy = false;
			ctx.latexDownloadMsg = null;
		}
	}

	// The backend serialises compile failures as JSON { message, log, diagnostics }
	// (line numbers already mapped to editor coordinates). Fall back to plain text.
	function parseLatexError(e: unknown): {
		message: string;
		diagnostics: { line: number; message: string; severity?: 'error' | 'warning' }[];
	} {
		const raw =
			typeof e === 'string' ? e : e instanceof Error ? e.message : JSON.stringify(e) || String(e);
		try {
			const parsed = JSON.parse(raw);
			if (parsed && Array.isArray(parsed.diagnostics)) {
				return {
					message: parsed.message ?? 'LaTeX compilation failed',
					diagnostics: parsed.diagnostics
				};
			}
		} catch {
			/* not structured — show the raw string */
		}
		return { message: raw, diagnostics: [] };
	}

	function closeTexPreview() {
		ctx.activeSourceBytes = null;
		ctx.activeSection = null;
		ctx.sectionCache = null;
		ctx.showAttachedNote = false;
	}

	// Debounced auto-compile: a couple of seconds after typing stops, when armed.
	$effect(() => {
		const body = ctx.draftBody;
		if (ctx.workingDocType === 'tex' && body !== ctx.lastTexBody) {
			ctx.lastTexBody = body;
			ctx.texRevision += 1;
			if (ctx.texAutoCompile && ctx.texCacheWarmed) ctx.texPreviewStatus = 'pending';
		}
		const armed = ctx.texAutoCompile && ctx.workingDocType === 'tex';
		if (!armed) return;
		if (ctx.texAutoTimer) clearTimeout(ctx.texAutoTimer);
		ctx.texAutoTimer = setTimeout(() => void compileTex(), 350);
		return () => {
			if (ctx.texAutoTimer) clearTimeout(ctx.texAutoTimer);
		};
	});

	return { pickLatexImage, compileTex, parseLatexError, closeTexPreview };
}
