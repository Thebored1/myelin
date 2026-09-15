import type { ControllerContext } from '$lib/controller-context';

/** Owns focus and selection preservation while dialogs temporarily take focus. */
export function createEditorInteractionSession(rawContext: object) {
	const ctx = rawContext as ControllerContext;
	let shortcutEditorTextOffset: number | null = null;
	function renderedTextOffset(editorEl: HTMLElement, range: Range): number | null {
		if (!editorEl.contains(range.startContainer)) return null;
		const prefix = document.createRange();
		prefix.selectNodeContents(editorEl);
		prefix.setEnd(range.startContainer, range.startOffset);
		return prefix.toString().length;
	}
	function restoreRenderedTextOffset(editorEl: HTMLElement, offset: number): boolean {
		const walker = document.createTreeWalker(editorEl, NodeFilter.SHOW_TEXT);
		let remaining = Math.max(0, offset);
		let last: Text | null = null;
		let node: Node | null;
		while ((node = walker.nextNode())) {
			last = node as Text;
			const length = node.textContent?.length ?? 0;
			if (remaining <= length) {
				const range = document.createRange();
				range.setStart(node, remaining);
				range.collapse(true);
				const selection = window.getSelection();
				if (!selection) return false;
				selection.removeAllRanges();
				selection.addRange(range);
				return true;
			}
			remaining -= length;
		}
		if (!last) return false;
		const range = document.createRange();
		range.setStart(last, last.length);
		range.collapse(true);
		const selection = window.getSelection();
		if (!selection) return false;
		selection.removeAllRanges();
		selection.addRange(range);
		return true;
	}
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
		const liveRange = selection?.rangeCount ? selection.getRangeAt(0) : null;
		if (
			editorEl &&
			ctx.savedEditorRange?.collapsed &&
			editorEl.contains(ctx.savedEditorRange.startContainer)
		) {
			ctx.shortcutEditorRange = ctx.savedEditorRange.cloneRange();
			shortcutEditorTextOffset = renderedTextOffset(editorEl, ctx.shortcutEditorRange);
			ctx.saveCursorPosition(ctx.shortcutEditorRange);
			ctx.captureEditorSelection(ctx.shortcutEditorRange);
			return;
		}
		if (
			editorEl &&
			(editorEl === document.activeElement || editorEl.contains(document.activeElement)) &&
			liveRange &&
			editorEl.contains(liveRange.commonAncestorContainer)
		) {
			ctx.shortcutEditorRange = liveRange.cloneRange();
			shortcutEditorTextOffset = liveRange.collapsed ? renderedTextOffset(editorEl, liveRange) : null;
			ctx.saveCursorPosition(liveRange);
			ctx.captureEditorSelection(liveRange);
			return;
		}
		if (!editorEl || !selection || selection.rangeCount === 0) return;
		const range = selection.getRangeAt(0);
		if (!editorEl.contains(range.commonAncestorContainer)) return;
		ctx.shortcutEditorRange = range.cloneRange();
		shortcutEditorTextOffset = renderedTextOffset(editorEl, range);
		// The shortcut focuses the chat textarea immediately after this call, so
		// preserve the visual editor caret before the browser moves the selection.
		ctx.saveCursorPosition(range);
		// Capture synchronously, before focusing the textarea changes the browser
		// selection, so write operations retain their cursor/selection target.
		ctx.captureEditorSelection(range);
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
		const rangeToRestore = ctx.shortcutEditorRange ?? ctx.savedEditorRange;
		if (
			editorEl &&
			selection &&
			rangeToRestore &&
			editorEl.contains(rangeToRestore.commonAncestorContainer)
		) {
			selection.removeAllRanges();
			selection.addRange(rangeToRestore);
		} else if (editorEl && shortcutEditorTextOffset !== null) {
			restoreRenderedTextOffset(editorEl, shortcutEditorTextOffset);
		}
		ctx.shortcutEditorRange = null;
		shortcutEditorTextOffset = null;
		ctx.clearSavedCaretProxy();
	}

	return {
		focusEditor,
		refocusEditorSoon,
		captureShortcutEditorTarget,
		restoreShortcutEditorFocus
	};
}
