import type { ControllerContext } from '$lib/controller-context';

/** Owns focus and selection preservation while dialogs temporarily take focus. */
export function createEditorInteractionSession(rawContext: object) {
	const ctx = rawContext as ControllerContext;
	function focusEditor() {
		if (!ctx.vditorInstance || !ctx.vditorContainer) return;
		ctx.vditorInstance.focus();
		const editorEl = ctx.vditorContainer.querySelector('.vditor-ir') as HTMLElement | null;
		editorEl?.focus();
	}

	function refocusEditorSoon() {
		ctx.shouldRefocusEditor = false;
		setTimeout(() => focusEditor(), 0);
	}

	function captureShortcutEditorTarget() {
		if (ctx.workingDocType !== 'md' || !ctx.vditorContainer) return;
		const editorEl = ctx.vditorContainer.querySelector('.vditor-ir') as HTMLElement | null;
		const selection = window.getSelection();
		if (!editorEl || !selection || selection.rangeCount === 0) return;
		const range = selection.getRangeAt(0);
		if (!editorEl.contains(range.commonAncestorContainer)) return;
		ctx.shortcutEditorRange = range.cloneRange();
		// Capture synchronously, before focusing the textarea changes the browser
		// selection, so write operations retain their cursor/selection target.
		ctx.captureEditorSelection();
	}

	function restoreShortcutEditorFocus() {
		if (ctx.workingDocType === 'tex') {
			ctx.texEditorInstance?.focusEditor?.();
			return;
		}
		if (ctx.workingDocType === 'ipynb') {
			ctx.ipynbEditorInstance?.focusEditor?.();
			return;
		}
		focusEditor();
		const editorEl = ctx.vditorContainer?.querySelector('.vditor-ir') as HTMLElement | null;
		const selection = window.getSelection();
		if (
			editorEl &&
			selection &&
			ctx.shortcutEditorRange &&
			editorEl.contains(ctx.shortcutEditorRange.commonAncestorContainer)
		) {
			selection.removeAllRanges();
			selection.addRange(ctx.shortcutEditorRange);
		}
		ctx.shortcutEditorRange = null;
	}

	return {
		focusEditor,
		refocusEditorSoon,
		captureShortcutEditorTarget,
		restoreShortcutEditorFocus
	};
}
