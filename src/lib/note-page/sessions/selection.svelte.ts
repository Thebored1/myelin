/** Editor selection and cursor mapping session. */
export function createSelectionSession(ctx: Record<string, any>) {
	type SelectionTarget = {
		text: string;
		before: string;
		after: string;
		cursor: boolean;
		cellIndex?: number;
	};
	function getSelectionTextOffset(editorEl: HTMLElement): number | null {
		const selection = window.getSelection();
		if (!selection || selection.rangeCount === 0) return null;
	
		const range = selection.getRangeAt(0);
		if (!editorEl.contains(range.endContainer)) return null;
	
		const walker = document.createTreeWalker(editorEl, NodeFilter.SHOW_TEXT);
		let offset = 0;
		let node: Node | null;
		while ((node = walker.nextNode())) {
			const textLength = node.textContent?.length ?? 0;
			if (node === range.endContainer) {
				return offset + range.endOffset;
			}
			offset += textLength;
		}
	
		return offset;
	}
	
	// Text offset of a (container, offset) point within the editor's rendered text.
	function textOffsetOf(editorEl: HTMLElement, container: Node, offset: number): number | null {
		const walker = document.createTreeWalker(editorEl, NodeFilter.SHOW_TEXT);
		let acc = 0;
		let node: Node | null;
		while ((node = walker.nextNode())) {
			if (node === container) return acc + offset;
			acc += node.textContent?.length ?? 0;
		}
		return null;
	}
	
	// Occurrence of `needle` in `hay` whose start is closest to `hint` (disambiguates repeats).
	function nearestIndexOf(hay: string, needle: string, hint: number): number {
		let best = -1;
		let bestDist = Infinity;
		let from = 0;
		let i: number;
		while ((i = hay.indexOf(needle, from)) >= 0) {
			const d = Math.abs(i - hint);
			if (d < bestDist) {
				bestDist = d;
				best = i;
			}
			from = i + 1;
		}
		return best;
	}
	
	// Map the current editor selection to a source-markdown span + surrounding
	// context. In Vditor IR mode the rendered text ≈ the source for prose, so the
	// tree-walked offsets usually map straight in; we validate and fall back to a
	// proximity text-search when formatting markers skew them.
	function computeSourceSelection(
		allowTextFallback = true
	): { text: string; before: string; after: string } | null {
		if (!ctx.vditorInstance || !ctx.vditorContainer) return null;
		const editorEl = ctx.vditorContainer.querySelector('.vditor-ir') as HTMLElement | null;
		if (!editorEl) return null;
		const sel = window.getSelection();
		if (!sel || sel.rangeCount === 0 || sel.isCollapsed) return null;
		const range = sel.getRangeAt(0);
		if (!editorEl.contains(range.commonAncestorContainer)) return null;
		const selText = sel.toString();
		if (!selText.trim()) return null;
	
		const source = ctx.vditorInstance.getValue();
		const startOff = textOffsetOf(editorEl, range.startContainer, range.startOffset);
		const endOff = textOffsetOf(editorEl, range.endContainer, range.endOffset);
	
		let s = -1;
		let e = -1;
		if (startOff != null && endOff != null && source.slice(startOff, endOff) === selText) {
			s = startOff;
			e = endOff;
		} else if (allowTextFallback) {
			s = nearestIndexOf(source, selText, startOff ?? 0);
			if (s >= 0) e = s + selText.length;
		}
		if (s < 0) return null;
	
		const N = 40;
		return {
			text: source.slice(s, e),
			before: source.slice(Math.max(0, s - N), s),
			after: source.slice(e, Math.min(source.length, e + N))
		};
	}
	
	function computeSourceCursor(): SelectionTarget | null {
		if (!ctx.vditorInstance || !ctx.vditorContainer) return null;
		const editorEl = ctx.vditorContainer.querySelector('.vditor-ir') as HTMLElement | null;
		const sel = window.getSelection();
		if (!editorEl || !sel || sel.rangeCount === 0 || !sel.isCollapsed) return null;
		const range = sel.getRangeAt(0);
		if (!editorEl.contains(range.startContainer)) return null;
	
		let renderedOffset = textOffsetOf(editorEl, range.startContainer, range.startOffset);
		if (renderedOffset == null) {
			try {
				const prefix = document.createRange();
				prefix.selectNodeContents(editorEl);
				prefix.setEnd(range.startContainer, range.startOffset);
				renderedOffset = prefix.toString().length;
			} catch {
				return null;
			}
		}
		const source = ctx.vditorInstance.getValue();
		if (!source.trim()) {
			return { text: '', before: '', after: '', cursor: true };
		}
		const position = Math.min(renderedOffset, source.length);
		const N = 80;
		return {
			text: '',
			before: source.slice(Math.max(0, position - N), position),
			after: source.slice(position, Math.min(source.length, position + N)),
			cursor: true
		};
	}
	
	function clearArmedSelection() {
		ctx.armedSelection = null;
	}
	
	// Keep the captured selection only while the user moves into the prompt.
	// Any other click clears it; a new editor drag captures a fresh selection.
	function onDocMouseDown(e: MouseEvent) {
		const target = e.target as HTMLElement | null;
		if (target?.closest('.prompt-box')) return;
		if (ctx.armedSelection) clearArmedSelection();
	}
	
	function captureEditorSelection() {
		if (!ctx.vditorContainer) return;
		const editorEl = ctx.vditorContainer.querySelector('.vditor-ir') as HTMLElement | null;
		if (!editorEl) return;
		const sel = window.getSelection();
		if (!sel || sel.rangeCount === 0) return;
		const range = sel.getRangeAt(0);
		if (!editorEl.contains(range.commonAncestorContainer)) return;
		if (sel.isCollapsed) {
			const computed = computeSourceCursor();
			if (computed) {
				ctx.armedSelection = { ...computed, chars: 0, words: 0 };
				ctx.writeTargetNotice = false;
			}
			return;
		}
		const computed = computeSourceSelection();
		if (computed) {
			const words = computed.text.trim().split(/\s+/).filter(Boolean).length;
			ctx.armedSelection = {
				...computed,
				cursor: false,
				chars: computed.text.length,
				words
			};
			ctx.writeTargetNotice = false;
		}
	}
	
	function captureExternalTarget(target: SelectionTarget | null) {
		if (!target) {
			clearArmedSelection();
			return;
		}
		const words = target.text.trim().split(/\s+/).filter(Boolean).length;
		ctx.armedSelection = {
			...target,
			chars: target.text.length,
			words
		};
		ctx.writeTargetNotice = false;
	}
	
	// After the AI edits the armed selection, update its source anchors so an
	// immediate follow-up can target the replacement without extra decoration.
	function reselectAfterEdit() {
		if (!ctx.armedSelection || !ctx.vditorInstance) return;
		const source = ctx.vditorInstance.getValue();
		const before = ctx.armedSelection.before;
		const after = ctx.armedSelection.after;
		let s = 0;
		let e = source.length;
		if (before) {
			const bi = source.indexOf(before);
			if (bi >= 0) s = bi + before.length;
		}
		if (after) {
			const ai = source.indexOf(after, s);
			if (ai >= 0) e = ai;
		}
		if (e <= s) return;
		const newText = source.slice(s, e);
		if (!newText.trim()) return;
		const words = newText.trim().split(/\s+/).filter(Boolean).length;
		ctx.armedSelection = {
			text: newText,
			before: source.slice(Math.max(0, s - 40), s),
			after: source.slice(e, Math.min(source.length, e + 40)),
			cursor: false,
			chars: newText.length,
			words
		};
	}
	
	function armedEditTarget(): SelectionTarget | null {
		if (!ctx.armedSelection) return null;
		const { chars: _chars, words: _words, ...target } = ctx.armedSelection;
		return target;
	}
	
	function onSelectionChange() {
		clearTimeout(ctx.selDebounce);
		ctx.selDebounce = setTimeout(captureEditorSelection, 120);
	}
	
	function restoreSelectionTextOffset(editorEl: HTMLElement, targetOffset: number) {
		const selection = window.getSelection();
		if (!selection) return;
	
		const walker = document.createTreeWalker(editorEl, NodeFilter.SHOW_TEXT);
		let offset = 0;
		let node: Node | null;
		while ((node = walker.nextNode())) {
			const textLength = node.textContent?.length ?? 0;
			const nextOffset = offset + textLength;
			if (targetOffset <= nextOffset) {
				const range = document.createRange();
				range.setStart(node, Math.max(0, targetOffset - offset));
				range.collapse(true);
				selection.removeAllRanges();
				selection.addRange(range);
				return;
			}
			offset = nextOffset;
		}
	
		editorEl.focus();
	}
	
	function saveCursorPosition() {
		if (!ctx.vditorInstance || !ctx.vditorContainer) return;
		const editorEl = ctx.vditorContainer.querySelector('.vditor-ir') as HTMLElement | null;
		const selection = window.getSelection();
		if (!editorEl || !selection || selection.rangeCount === 0) return;
	
		const range = selection.getRangeAt(0);
		if (!editorEl.contains(range.commonAncestorContainer)) return;
	
		ctx.savedEditorRange = range.cloneRange();
	}
	
	function insertAtSavedCursor(linkText: string) {
		if (!ctx.vditorInstance || !ctx.vditorContainer) return;
		const editorEl = ctx.vditorContainer.querySelector('.vditor-ir') as HTMLElement | null;
		if (!editorEl) return;
	
		ctx.focusEditor();
	
		const selection = window.getSelection();
		if (ctx.savedEditorRange && selection) {
			selection.removeAllRanges();
			selection.addRange(ctx.savedEditorRange);
		}
	
		const inserted = document.execCommand('insertText', false, linkText);
		if (!inserted) {
			ctx.vditorInstance.insertValue(linkText, true);
		}
	
		ctx.savedEditorRange = null;
		ctx.focusEditor();
		ctx.draftBody = ctx.vditorInstance.getValue();
		ctx.triggerAutoSave();
	}
	

	return {
		getSelectionTextOffset,
		textOffsetOf,
		nearestIndexOf,
		computeSourceSelection,
		computeSourceCursor,
		clearArmedSelection,
		onDocMouseDown,
		captureEditorSelection,
		captureExternalTarget,
		reselectAfterEdit,
		armedEditTarget,
		onSelectionChange,
		restoreSelectionTextOffset,
		saveCursorPosition,
		insertAtSavedCursor
	};
}
