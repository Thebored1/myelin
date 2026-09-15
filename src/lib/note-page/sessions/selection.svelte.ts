import type { ControllerContext } from '$lib/controller-context';

/** Editor selection and cursor mapping session. */
export function createSelectionSession(rawContext: object) {
	const ctx = rawContext as ControllerContext;
	let savedCaretProxy: HTMLSpanElement | null = null;
	let savedCaretUpdate: (() => void) | null = null;
	let savedCaretResizeObserver: ResizeObserver | null = null;
	let lastSavedCaretRect: { left: number; top: number; height: number } | null = null;
	let cursorCaptureVersion = 0;
	let pendingEditorClick: { x: number; y: number } | null = null;

	function clearSavedCaretProxy() {
		if (savedCaretUpdate) {
			window.removeEventListener('resize', savedCaretUpdate);
			ctx.vditorContainer?.removeEventListener('scroll', savedCaretUpdate, true);
			savedCaretUpdate = null;
		}
		savedCaretResizeObserver?.disconnect();
		savedCaretResizeObserver = null;
		savedCaretProxy?.remove();
		savedCaretProxy = null;
		lastSavedCaretRect = null;
	}

	function updateSavedCaretProxy() {
		if (!savedCaretProxy || !ctx.savedEditorRange || !ctx.vditorContainer) return;
		const editorEl = ctx.vditorContainer.querySelector('.vditor-ir') as HTMLElement | null;
		// Vditor can rebuild its contenteditable while focus is in the prompt.
		// Keep the last known marker position during that brief transition rather
		// than deleting the only visible indication of the write target.
		if (!editorEl || !editorEl.contains(ctx.savedEditorRange.startContainer)) return;
		const rangeRect =
			ctx.savedEditorRange.getClientRects()[0] ?? ctx.savedEditorRange.getBoundingClientRect();
		if (!rangeRect.height && !rangeRect.width) return;
		lastSavedCaretRect = {
			left: rangeRect.left,
			top: rangeRect.top,
			height: Math.max(rangeRect.height, 18)
		};
		savedCaretProxy.style.left = `${lastSavedCaretRect.left}px`;
		savedCaretProxy.style.top = `${lastSavedCaretRect.top}px`;
		savedCaretProxy.style.height = `${lastSavedCaretRect.height}px`;
	}

	function showSavedCaretProxy() {
		if (!ctx.savedEditorRange?.collapsed || !ctx.vditorContainer) {
			clearSavedCaretProxy();
			return;
		}
		if (!savedCaretProxy) {
			savedCaretProxy = document.createElement('span');
			savedCaretProxy.className = 'myelin-editor-caret-proxy';
			savedCaretProxy.setAttribute('aria-hidden', 'true');
			// Keep the marker outside Vditor's mount tree. Vditor is allowed to
			// replace that tree when the note/sidebar state updates, which used to
			// make the caret disappear as soon as the Cursor pill rendered.
			document.body.appendChild(savedCaretProxy);
			savedCaretUpdate = updateSavedCaretProxy;
			window.addEventListener('resize', savedCaretUpdate);
			ctx.vditorContainer.addEventListener('scroll', savedCaretUpdate, true);
			if (typeof ResizeObserver !== 'undefined') {
				savedCaretResizeObserver = new ResizeObserver(savedCaretUpdate);
				savedCaretResizeObserver.observe(ctx.vditorContainer);
			}
		}
		updateSavedCaretProxy();
		requestAnimationFrame(() => {
			if (ctx.savedEditorRange?.collapsed) updateSavedCaretProxy();
		});
	}

	type SelectionTarget = {
		text: string;
		before: string;
		after: string;
		cursor: boolean;
		sourceOffset?: number;
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

	// Return the rendered-text offset of a DOM range point. Range.toString() is
	// inconsistent in Vditor IR when the point is inside the last block, so walk
	// the text nodes instead of asking the browser to serialize a cross-block
	// range.
	function renderedTextOffsetAt(editorEl: HTMLElement, container: Node, offset: number): number | null {
		if (!editorEl.contains(container)) return null;

		function measure(node: Node): { found: boolean; length: number } {
			if (node === container) {
				if (node.nodeType === Node.TEXT_NODE) {
					return { found: true, length: Math.min(offset, node.textContent?.length ?? 0) };
				}
				let length = 0;
				for (let index = 0; index < Math.min(offset, node.childNodes.length); index += 1) {
					length += node.childNodes[index]?.textContent?.length ?? 0;
				}
				return { found: true, length };
			}
			if (node.nodeType === Node.TEXT_NODE) return { found: false, length: node.textContent?.length ?? 0 };

			let length = 0;
			for (const child of Array.from(node.childNodes)) {
				const result = measure(child);
				if (result.found) return { found: true, length: length + result.length };
				length += result.length;
			}
			return { found: false, length };
		}

		const result = measure(editorEl);
		return result.found ? result.length : null;
	}

	function rangeAtRenderedOffset(editorEl: HTMLElement, targetOffset: number): Range | null {
		const walker = document.createTreeWalker(editorEl, NodeFilter.SHOW_TEXT);
		let offset = Math.max(0, targetOffset);
		let last: Text | null = null;
		let node: Node | null;
		while ((node = walker.nextNode())) {
			last = node as Text;
			const length = node.textContent?.length ?? 0;
			if (offset <= length) {
				const range = document.createRange();
				range.setStart(node, offset);
				range.collapse(true);
				return range;
			}
			offset -= length;
		}
		if (!last) return null;
		const range = document.createRange();
		range.setStart(last, last.length);
		range.collapse(true);
		return range;
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

	function computeSourceCursor(rangeOverride?: Range): SelectionTarget | null {
		if (!ctx.vditorInstance || !ctx.vditorContainer) return null;
		const editorEl = ctx.vditorContainer.querySelector('.vditor-ir') as HTMLElement | null;
		const sel = window.getSelection();
		const range = rangeOverride ?? (sel?.rangeCount ? sel.getRangeAt(0) : null);
		if (!editorEl || !range || !range.collapsed) return null;
		if (!editorEl.contains(range.startContainer)) return null;

		const source = ctx.vditorInstance.getValue();
		if (!source.trim()) {
			return { text: '', before: '', after: '', cursor: true, sourceOffset: 0 };
		}
		// Vditor IR is rendered Markdown, not a 1:1 source representation. A
		// temporary marker gives its own Markdown serializer the exact source
		// boundary for this DOM caret, including hidden line breaks and markers.
		const marker = `MYELINCURSOR${Date.now()}${Math.random().toString(36).slice(2)}`;
		const markerNode = document.createTextNode(marker);
		const probeRange = range.cloneRange();
		try {
			probeRange.insertNode(markerNode);
			const sourceWithMarker = ctx.vditorInstance.getValue();
			const markerOffset = sourceWithMarker.indexOf(marker);
			if (markerOffset >= 0) {
				const N = 80;
				return {
					text: '',
					before: sourceWithMarker.slice(Math.max(0, markerOffset - N), markerOffset),
					after: sourceWithMarker.slice(markerOffset + marker.length, markerOffset + marker.length + N),
					cursor: true,
					sourceOffset: markerOffset
				};
			}
		} finally {
			markerNode.remove();
			if (sel) {
				sel.removeAllRanges();
				sel.addRange(range);
			}
		}
		// Do not use a rendered character offset as a source offset. Vditor IR
		// omits Markdown newlines/markers from its DOM, so that offset drifts as
		// soon as the caret crosses more than one rendered block. Capture the
		// actual DOM text on both sides of the caret instead; the Rust locator
		// matches this context whitespace-tolerantly against source Markdown.
		const renderedText = editorEl.textContent ?? '';
		const renderedOffset = renderedTextOffsetAt(editorEl, range.startContainer, range.startOffset);
		if (renderedOffset == null) return null;
		const N = 80;
		const beforeContext = renderedText.slice(Math.max(0, renderedOffset - N), renderedOffset);
		const afterContext = renderedText.slice(renderedOffset, renderedOffset + N);
		return {
			text: '',
			before: beforeContext,
			after: afterContext,
			cursor: true
		};
	}

	function sameCollapsedRange(left: Range | null, right: Range | null): boolean {
		return Boolean(
			left?.collapsed &&
			right?.collapsed &&
			left.startContainer === right.startContainer &&
			left.startOffset === right.startOffset
		);
	}

	function rangeAtPoint(editorEl: HTMLElement, x: number, y: number): Range | null {
		const pointDocument = document as Document & {
			caretPositionFromPoint?: (x: number, y: number) => { offsetNode: Node; offset: number } | null;
			caretRangeFromPoint?: (x: number, y: number) => Range | null;
		};
		const range = pointDocument.caretRangeFromPoint?.(x, y) ?? null;
		if (range && editorEl.contains(range.startContainer)) return range;
		const position = pointDocument.caretPositionFromPoint?.(x, y);
		if (position && position.offsetNode.nodeType === Node.TEXT_NODE && editorEl.contains(position.offsetNode)) {
			const fallbackRange = document.createRange();
			fallbackRange.setStart(position.offsetNode, position.offset);
			fallbackRange.collapse(true);
			return fallbackRange;
		}

		// Some WebKit builds expose the point APIs but return the editor element
		// itself for contenteditable clicks. Resolve the nearest text caret from
		// geometry as a fallback; this runs only for the completed click, not on
		// every selectionchange.
		const walker = document.createTreeWalker(editorEl, NodeFilter.SHOW_TEXT);
		let best: { range: Range; score: number } | null = null;
		let node: Node | null;
		while ((node = walker.nextNode())) {
			const text = node.textContent ?? '';
			for (let offset = 0; offset <= text.length; offset += 1) {
				const candidate = document.createRange();
				candidate.setStart(node, offset);
				candidate.collapse(true);
				const rect = candidate.getBoundingClientRect();
				if (!rect.height) continue;
				const verticalDistance = y < rect.top ? rect.top - y : y > rect.bottom ? y - rect.bottom : 0;
				const score = verticalDistance * 20 + Math.abs(rect.left - x);
				if (!best || score < best.score) best = { range: candidate.cloneRange(), score };
			}
		}
		return best?.range ?? null;
	}

	function clearArmedSelection() {
		ctx.armedSelection = null;
	}

	// Keep the captured selection only while the user moves into the prompt.
	// Sidebar controls are an intentional exception: clicking them should not
	// destroy the editor's caret/selection target. The DOM focus moves to the
	// control so it remains usable, while the Range is saved for later inserts.
	function onDocMouseDown(e: MouseEvent) {
		const target = e.target as HTMLElement | null;
		if (target?.closest('.prompt-box, .sidebar, .sidebar-backdrop')) {
			saveCursorPosition();
			return;
		}
		if (target?.closest('.vditor-ir')) {
			if (ctx.armedSelection) clearArmedSelection();
			clearSavedCaretProxy();
			pendingEditorClick = { x: e.clientX, y: e.clientY };
			const captureAfterClick = () => {
				const click = pendingEditorClick;
				pendingEditorClick = null;
				if (!ctx.noteStreaming) {
					const selection = window.getSelection();
					if (!selection || selection.isCollapsed) {
						const editorEl = ctx.vditorContainer?.querySelector('.vditor-ir') as HTMLElement | null;
						const pointRange = click && editorEl ? rangeAtPoint(editorEl, click.x, click.y) : null;
						captureEditorSelection(pointRange ?? undefined);
					}
				}
			};
			// mousedown fires before the browser commits the point-based caret. A
			// frame scheduled here can therefore capture Vditor's block boundary
			// (usually offset 0), even though the native caret moves correctly.
			// Capture after mouseup instead, when the browser and Vditor agree on
			// the live editor range. The listener is one-shot for this click.
			window.addEventListener('mouseup', captureAfterClick, { once: true });
			return;
		}
		if (ctx.armedSelection) clearArmedSelection();
	}

	function captureEditorSelection(rangeOverride?: Range) {
		if (!ctx.vditorContainer) return;
		const editorEl = ctx.vditorContainer.querySelector('.vditor-ir') as HTMLElement | null;
		if (!editorEl) return;
		const sel = window.getSelection();
		const range = rangeOverride ?? (sel?.rangeCount ? sel.getRangeAt(0) : null);
		if (!range) return;
		if (!editorEl.contains(range.commonAncestorContainer)) return;
		if (range.collapsed) {
			const renderedOffset = renderedTextOffsetAt(editorEl, range.startContainer, range.startOffset);
			const savedRange = (renderedOffset === null ? range : rangeAtRenderedOffset(editorEl, renderedOffset) ?? range).cloneRange();
			if (sameCollapsedRange(ctx.savedEditorRange, savedRange) && ctx.armedSelection?.cursor) return;
			ctx.savedEditorRange = savedRange;
			const captureVersion = ++cursorCaptureVersion;
			requestAnimationFrame(() => {
				if (captureVersion !== cursorCaptureVersion || ctx.savedEditorRange !== savedRange) return;
				const currentEditor = ctx.vditorContainer?.querySelector('.vditor-ir') as HTMLElement | null;
				if (!currentEditor?.contains(savedRange.startContainer)) return;
				const computed = computeSourceCursor(savedRange);
				if (computed) {
					ctx.armedSelection = { ...computed, chars: 0, words: 0 };
					ctx.writeTargetNotice = false;
				}
			});
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
		const { chars, words, ...target } = ctx.armedSelection;
		void chars;
		void words;
		return target;
	}

	function onSelectionChange() {
		clearTimeout(ctx.selDebounce);
		ctx.selDebounce = setTimeout(captureEditorSelection, 120);
	}

	function restoreSelectionTextOffset(editorEl: HTMLElement, targetOffset: number) {
		clearSavedCaretProxy();
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
				ctx.savedEditorRange = range.cloneRange();
				showSavedCaretProxy();
				return;
			}
			offset = nextOffset;
		}

		editorEl.focus();
	}

	function saveCursorPosition(rangeOverride?: Range) {
		if (!ctx.vditorInstance || !ctx.vditorContainer) return;
		// Once the composer owns focus, the window selection is no longer the
		// editor's caret. Preserve the last editor range instead of replacing it
		// with a stale/global range from the prompt interaction.
		if (!rangeOverride && ctx.chatTextareaEl === document.activeElement) return;
		const editorEl = ctx.vditorContainer.querySelector('.vditor-ir') as HTMLElement | null;
		const selection = window.getSelection();
		if (!editorEl) return;

		const range = rangeOverride ?? (selection?.rangeCount ? selection.getRangeAt(0) : null);
		if (!range) return;
		if (!editorEl.contains(range.commonAncestorContainer)) return;

		ctx.savedEditorRange = range.cloneRange();
		showSavedCaretProxy();
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
		clearSavedCaretProxy();
		ctx.focusEditor();
		ctx.draftBody = ctx.vditorInstance.getValue();
		ctx.triggerAutoSave();
	}

	function handleGlobalSelectionChange() {
		// Streaming rebuilds the editor DOM and restores the caret programmatically,
		// which fires selectionchange; skip the expensive capture until it settles.
		if (ctx.noteStreaming || !ctx.vditorContainer) return;
		const selection = window.getSelection();
		ctx.vditorContainer.querySelectorAll('.force-expand').forEach((element: Element) => {
			element.classList.remove('force-expand');
		});
		if (!selection || selection.rangeCount === 0) return;
		// Do not clear the proxy here. A selectionchange can be emitted while the
		// prompt takes focus, even though the saved editor target is still valid.
		// Actual editor clicks and completed insertions clear it explicitly.
		onSelectionChange();
		if (selection.isCollapsed) return;
		ctx.vditorContainer.querySelectorAll('[data-type="a"]').forEach((link: Element) => {
			if (selection.containsNode(link, true)) link.classList.add('force-expand');
		});
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
		clearSavedCaretProxy,
		insertAtSavedCursor,
		handleGlobalSelectionChange
	};
}
