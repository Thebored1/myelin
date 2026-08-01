<script lang="ts">
	import { invoke } from '@tauri-apps/api/core';
	import { listen, type UnlistenFn } from '@tauri-apps/api/event';
	import { open as openFileDialog } from '@tauri-apps/plugin-dialog';
	import { goto, beforeNavigate } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import type {
		NoteDocument,
		SearchResponse,
		NoteSummary,
		PdfAnnotation,
		GitCommit,
		ChatMessage
	} from '$lib/types';
	import { onMount, onDestroy, tick } from 'svelte';
	import { noteOpened, noteClosed } from '$lib/llamaWarm';
	import { chatSidebarShortcut, showSidebarToggle, noteSidebarOpen } from '$lib/stores';
	import { shortcutMatches } from '$lib/keyboardShortcut';
	import { theme } from '$lib/theme';
	import type Vditor from 'vditor';
	import 'mathlive/fonts.css';

	import ChatToolIndicator from '$lib/components/ChatToolIndicator.svelte';
	import { hideThinkingContent } from '$lib/chatContent';
	import {
		composeNoteStreamPreviewWithStatus,
		locateNoteStreamTarget
	} from '$lib/noteStreamPreview';
	import { resolveActiveAiTarget } from '$lib/aiTarget';
	import {
		canApplyReconciledNote,
		editorNeedsAuthoritativeBody,
		hasNoteMutation
	} from '$lib/noteMutation';
	import { marked } from 'marked';
	import DOMPurify from 'dompurify';

	let requireToolApproval = $state(false);
	let note = $state<NoteDocument | null>(null);
	let isLoadingNote = $state(false);
	let draftBody = $state('');
	let draftTitle = $state('');
	let draftTags = $state('');
	let isBusy = $state(false);
	let message = $state('');
	// First LaTeX compile fetches Tectonic's ~50 MB bundle; show real progress.
	let latexDownloadMsg = $state<string | null>(null);
	// .tex live-preview state.
	let texAutoCompile = $state(false);
	let texCompiling = $state(false);
	let texCacheWarmed = $state(false);
	let texPreviewStatus = $state<'pending' | 'compiling' | 'current' | 'error' | null>(null);
	let texCompileError = $state<string | null>(null);
	let texRevision = 0;
	let texCompileQueued = false;
	let lastTexBody = '';
	let texDiagnostics = $state<{ line: number; message: string; severity?: 'error' | 'warning' }[]>(
		[]
	);
	let texAutoTimer: ReturnType<typeof setTimeout> | undefined;

	let activeSidebarTab = $state<'info' | 'chat' | 'versions'>('info');
	let noteHistory = $state<GitCommit[]>([]);
	let versionPreviewContent = $state<string | null>(null);
	let versionPreviewHash = $state<string | null>(null);
	let versionPreviewDialog: HTMLDialogElement | undefined = $state();
	type NoteSnapshot = import('$lib/types').NoteSnapshot;

	let chatMessages = $state<ChatMessage[]>([]);
	let chatInput = $state('');
	let copiedIdx = $state<number | null>(null);
	// Coalesce the per-token ai://chat_chunk events into one chatMessages update
	// per frame, so the streaming bubble re-renders at most 60×/s instead of once
	// per token (each update re-parses the accumulated markdown).
	let chatChunkBuf = '';
	let chatChunkFlushPending = false;
	let chatPersistTimer: ReturnType<typeof setTimeout> | undefined;

	function persistableChatHistory(messages: ChatMessage[]): ChatMessage[] {
		return messages
			.filter(
				(message) =>
					message.role === 'user' ||
					!!message.content.trim() ||
					!!message.tools?.length ||
					message.error === true
			)
			.map(({ statusText: _statusText, ...message }) => ({
				...message,
				isStreaming: false
			}));
	}

	async function persistChatHistory(noteId = activeAiNoteId(), messages = chatMessages) {
		if (!noteId) return;
		try {
			const persisted = persistableChatHistory(messages);
			await invoke('save_chat_history', { noteId, chatHistory: persisted });
		} catch (error) {
			console.error('Failed to persist chat history:', error);
		}
	}

	function checkpointChatHistory(delay = 250) {
		if (chatPersistTimer) clearTimeout(chatPersistTimer);
		chatPersistTimer = setTimeout(() => {
			chatPersistTimer = undefined;
			void persistChatHistory();
		}, delay);
	}

	// Apply any chat deltas buffered since the last frame (coalesces the
	// per-token ai://chat_chunk events into one chatMessages update).
	function flushChatChunks() {
		if (!chatChunkBuf) return;
		const delta = chatChunkBuf;
		chatChunkBuf = '';
		chatMessages = chatMessages.map((m) => {
			if (m.isStreaming) {
				return { ...m, content: m.content + delta, statusText: undefined };
			}
			return m;
		});
		checkpointChatHistory();
		if (showDebugWindow && debugInfo) {
			if (debugInfo.firstChunk === null) {
				debugInfo = {
					...debugInfo,
					firstChunk: Date.now(),
					trace: [
						...debugInfo.trace,
						{ time: Date.now(), msg: 'Generation started', kind: 'gen' as const }
					]
				};
			}
			debugInfo = { ...debugInfo, replyChars: debugInfo.replyChars + delta.length };
		}
	}

	// Debug window state for AI performance metrics. Off by default — it renders
	// a live per-request trace (including full model prompts) that churns the
	// page for every user if left on.
	let showDebugWindow = $state(localStorage.getItem('myelin_debug_window') === 'true');
	$effect(() => {
		localStorage.setItem('myelin_debug_window', String(showDebugWindow));
	});
	type DebugTraceEntry = { time: number; msg: string; kind: string };
	// Keep the trace bounded: full model prompts are multi-KB and would otherwise
	// bloat every persisted chat message and the live debug window.
	const MAX_DEBUG_TRACE = 200;
	const MAX_DEBUG_MSG_CHARS = 2000;
	function makeDebugTraceEntry(kind: string, msg: string): DebugTraceEntry {
		let display = msg;
		if (display.length > MAX_DEBUG_MSG_CHARS) {
			display =
				display.slice(0, MAX_DEBUG_MSG_CHARS) + `… (+${display.length - MAX_DEBUG_MSG_CHARS}c)`;
		}
		return { time: Date.now(), msg: `[${kind}] ${display}`, kind };
	}

	// Memoized markdown render for chat bubbles. Each ai://chat_chunk re-renders
	// the streaming bubble, so without a cache the whole accumulated response is
	// parsed + sanitized on every token (O(n²) over the stream). Caching by exact
	// content means only the bubble whose content actually changed re-parses.
	const chatRenderCache = new Map<string, string>();
	const MAX_CHAT_RENDER_CACHE = 64;
	function renderChatContent(content: string): string {
		const cached = chatRenderCache.get(content);
		if (cached !== undefined) return cached;
		const rendered = DOMPurify.sanitize(marked.parse(content) as string);
		if (chatRenderCache.size >= MAX_CHAT_RENDER_CACHE && chatRenderCache.size > 0) {
			const oldest = chatRenderCache.keys().next().value as string | undefined;
			if (oldest) chatRenderCache.delete(oldest);
		}
		chatRenderCache.set(content, rendered);
		return rendered;
	}
	let pendingDebugTrace = $state<DebugTraceEntry[]>([]);
	let activeAiComposerMode: 'chat' | 'editor' | null = null;
	let activeChatNoteId: string | null = null;
	type AiInteractionMode = 'chat' | 'operation';
	let aiInteractionMode = $state<AiInteractionMode>('chat');

	function setAiInteractionMode(mode: AiInteractionMode) {
		aiInteractionMode = mode;
		if (mode === 'chat') writeTargetNotice = false;
		localStorage.setItem('myelin_ai_interaction_mode', mode);
	}

	function setToolApproval(require: boolean) {
		requireToolApproval = require;
		void invoke('set_require_tool_approval', { require });
	}

	function setStreamingStatus(statusText: string | undefined) {
		const changed = chatMessages.some(
			(message) => message.isStreaming && message.statusText !== statusText
		);
		if (!changed) return;
		chatMessages = chatMessages.map((message) =>
			message.isStreaming ? { ...message, statusText } : message
		);
		if (chatMessagesEl) setTimeout(() => scrollChatToBottom(false), 0);
	}

	function visibleAiStatus(kind: string, detail: string): string | undefined {
		if (kind === 'model_prompt' || kind === 'request_serialized') return 'Reading the note…';
		if (kind === 'response_headers' || kind === 'first_model_delta' || kind === 'gen') {
			return activeAiComposerMode === 'editor' ? 'Writing replacement…' : 'Writing a response…';
		}
		if (kind === 'intent_prompt') return 'Understanding the request…';
		if (kind === 'tool') {
			const name = detail.match(/executing\s+([^(]+)/i)?.[1]?.replaceAll('_', ' ');
			if (activeAiComposerMode === 'editor' && name?.trim() === 'write note') {
				return 'Applying selected edit…';
			}
			return name ? `Using ${name}…` : 'Looking that up…';
		}
		if (kind === 'tool_result') return 'Reading the result…';
		if (kind === 'session' || kind === 'config' || kind === 'tools' || kind === 'wire_mode') {
			return 'Preparing the request…';
		}
		return undefined;
	}
	let debugInfo = $state<{
		requestStart: number | null;
		firstChunk: number | null;
		generationStart: number | null;
		generationEnd: number | null;
		done: number | null;
		promptTokens: number;
		completionTokens: number;
		totalTokens: number;
		turnCount: number;
		replyChars: number;
		trace: DebugTraceEntry[];
	} | null>(null);

	async function copyMessage(idx: number, text: string) {
		try {
			await navigator.clipboard.writeText(text);
			copiedIdx = idx;
			setTimeout(() => {
				if (copiedIdx === idx) copiedIdx = null;
			}, 1200);
		} catch {
			/* clipboard unavailable */
		}
	}
	// The editor selection the user has "armed" for the AI. Persists across sends
	// (cleared only by the ✕ pill or by deselecting inside the editor). Captured in
	// source-markdown coordinates with surrounding context so the backend can pin
	// the exact span even as the note drifts.
	let armedSelection = $state<{
		text: string;
		before: string;
		after: string;
		cursor: boolean;
		cellIndex?: number;
		chars: number;
		words: number;
	} | null>(null);
	let selDebounce: ReturnType<typeof setTimeout> | undefined;
	type AiEditTarget = {
		text: string;
		before: string;
		after: string;
		cursor: boolean;
		cellIndex?: number;
	};
	let activeAiEditTarget: AiEditTarget | null = null;
	let writeTargetNotice = $state(false);

	// A chat turn is in flight while the last assistant bubble is still streaming.
	// Sending is blocked until it finishes, but the textarea stays editable so you
	// can compose your next prompt while the model is still answering.
	let activeChatRequestId: string | null = null;
	let isChatStreaming = $derived(chatMessages.some((m) => m.isStreaming));

	// The notebook (top-level folder) the open note lives in. Anything created or
	// uploaded while it's open inherits it, so docs stay with their note.
	function openNoteNotebook(): string | null {
		if (!note) return null;
		const segs = note.relativePath.replace(/\\/g, '/').split('/').filter(Boolean);
		return segs.length > 1 ? segs[0] : null;
	}

	// Upload button: attach a document (becomes a note via the PDF/EPUB import).
	async function attachFile() {
		const picked = await openFileDialog({
			multiple: false,
			filters: [{ name: 'Documents', extensions: ['pdf', 'epub'] }]
		});
		if (typeof picked === 'string') {
			try {
				await invoke('import_pdf_file', { filePath: picked, notebook: openNoteNotebook() });
			} catch (e) {
				console.error('attach failed', e);
			}
		}
	}
	let chatTextareaEl: HTMLTextAreaElement | undefined = $state();
	let chatMessagesEl: HTMLDivElement | undefined = $state();
	let currentTime = $state(Date.now());
	let debugTimer: ReturnType<typeof setInterval> | null = null;
	// Live "time taken" only matters while a message is streaming; run the
	// 100ms ticker only then instead of forever.
	$effect(() => {
		const streaming = chatMessages.some((m) => m.isStreaming);
		if (!streaming) return;
		const ticker = setInterval(() => {
			currentTime = Date.now();
		}, 100);
		return () => clearInterval(ticker);
	});

	let backUrl = $derived(page.url.searchParams.get('returnTo') || '/');

	let relatedNotes = $state<NoteSummary[]>([]);
	let vditorContainer: HTMLElement | undefined = $state();
	let vditorInstance: Vditor | null = null;
	let VditorConstructor = $state<any>(null);
	let vditorLoading = false;
	// Keep the note render separate from the editor/tool bundle. The bundle is
	// requested only after the note has had a chance to paint.
	let toolsReady = $state(false);
	let fullscreenShortcut = $state('Esc');
	// Live note streaming (real token-by-token writes from the backend).
	let noteStreaming = $state(false);
	let noteStreamBuf = '';
	let noteStreamBackup = '';
	// Coalesces the per-token note_delta events into one editor rebuild per
	// frame. setValue re-parses and re-renders the ENTIRE note, so applying it
	// on every token makes long writes visibly janky (and destroys the caret).
	let noteStreamFlushPending = false;
	// The target span never moves between deltas of one request; locate it once
	// in beginNoteStream instead of re-scanning the whole note per token.
	let noteStreamSpan: [number, number] | null = null;
	let savedEditorRange: Range | null = null;
	let shortcutEditorRange: Range | null = null;
	let shouldRefocusEditor = false;

	let isSourceMaterial = $state(false);
	let sourceMaterialType = $state<'pdf' | 'epub' | 'html' | null>(null);
	let workingDocType = $state<'md' | 'tex' | 'ipynb'>('md');
	// Optional document viewers/editors stay out of the Markdown cold path. They
	// are loaded only when the current document actually needs them.
	let PdfViewerComponent = $state<any>(null);
	let EpubViewerComponent = $state<any>(null);
	let HtmlViewerComponent = $state<any>(null);
	let TexEditorComponent = $state<any>(null);
	let IpynbEditorComponent = $state<any>(null);
	let texEditorInstance: { focusEditor?: () => void } | undefined = $state();
	let ipynbEditorInstance: { focusEditor?: () => void } | undefined = $state();
	let activeSourceId = $state<string | null>(null);
	let activeSourceBytes = $state<Uint8Array | null>(null);
	let scratchpadSavedId = $state<string | null>(null);
	let showAttachedNote = $state(false);
	let pdfIngestionStatus = $state<'idle' | 'indexing' | 'cached' | 'indexed' | 'empty' | 'failed'>(
		'idle'
	);
	let pdfIngestionError = $state<string | null>(null);
	let pdfIngestionPromise: Promise<void> | null = null;

	let splitRatio = $state(50);
	let isResizing = $state(false);
	let mainLayoutEl: HTMLElement | undefined = $state();

	function activeAiNoteId(): string | null {
		return (
			resolveActiveAiTarget({
				openedDocumentId: note?.id ?? null,
				isSourceMaterial,
				attachedNoteVisible: showAttachedNote,
				workingNoteId: scratchpadSavedId,
				attachedSourceId: activeSourceId
			})?.workingNoteId ?? null
		);
	}

	async function openAttachedNote() {
		if (!note || !isSourceMaterial) return;
		if (!scratchpadSavedId) {
			const created = await invoke<NoteDocument>('create_note', {
				title: draftTitle,
				sourcePdf: activeSourceId,
				notebook: openNoteNotebook()
			});
			scratchpadSavedId = created.id;
			draftTitle = created.title;
			draftBody = created.body;
			draftTags = created.tags.join(', ');
			chatMessages = created.chatHistory || [];
		} else {
			const workingNote = await invoke<NoteDocument>('load_note', { noteId: scratchpadSavedId });
			draftTitle = workingNote.title;
			draftBody = workingNote.body;
			draftTags = workingNote.tags.join(', ');
			chatMessages = workingNote.chatHistory || [];
		}
		showAttachedNote = true;
		noteOpened(scratchpadSavedId, 'chat');
		await tick();
		setTimeout(() => initVditor(), 100);
	}

	const PANE_MIN_WIDTH = 26 * 16;
	let sidebarWidth = $state(320);
	let isSidebarResizing = $state(false);

	function startSidebarResizing(e: MouseEvent) {
		e.preventDefault();
		isSidebarResizing = true;
	}

	function startResizing(e: MouseEvent) {
		e.preventDefault();
		window.getSelection()?.removeAllRanges();
		isResizing = true;
	}

	function handleGlobalMouseMove(e: MouseEvent) {
		if (isResizing && mainLayoutEl) {
			e.preventDefault();
			const rect = mainLayoutEl.getBoundingClientRect();
			// splitRatio is the LEFT (PDF) pane's width %, and the panes are in
			// natural order (PDF left, editor right), so the cursor's fraction from
			// the left edge is the ratio directly — no per-doc-type inversion.
			const newRatio = ((e.clientX - rect.left) / rect.width) * 100;
			const resizerWidth = 10;
			const minSourceWidth = PANE_MIN_WIDTH;
			const maxSourceRatio = ((rect.width - PANE_MIN_WIDTH - resizerWidth) / rect.width) * 100;
			const minSourceRatio = (minSourceWidth / rect.width) * 100;
			if (maxSourceRatio >= minSourceRatio) {
				splitRatio = Math.max(minSourceRatio, Math.min(newRatio, maxSourceRatio));
			}
		} else if (isSidebarResizing) {
			const newWidth = window.innerWidth - e.clientX;
			const containerWidth = mainLayoutEl?.getBoundingClientRect().width ?? window.innerWidth;
			const maxSidebar = Math.max(320, containerWidth - PANE_MIN_WIDTH);
			sidebarWidth = Math.max(320, Math.min(newWidth, maxSidebar));
		}
	}

	function stopResizing() {
		if (isResizing || isSidebarResizing) {
			isResizing = false;
			if (isSidebarResizing) {
				isSidebarResizing = false;
				localStorage.setItem('myelin_sidebar_width', sidebarWidth.toString());
			}
			if (vditorInstance) {
				// Let Vditor resize after layout shift
				setTimeout(() => {
					window.dispatchEvent(new Event('resize'));
				}, 50);
			}
		}
	}

	function handlePdfQuote(text: string, page: number) {
		appendToNoteBody(`\n> ${text}\n> *(Page ${page})*\n\n`);
	}

	function focusEditor() {
		if (!vditorInstance || !vditorContainer) return;
		vditorInstance.focus();
		const editorEl = vditorContainer.querySelector('.vditor-ir') as HTMLElement | null;
		editorEl?.focus();
	}

	function refocusEditorSoon() {
		shouldRefocusEditor = false;
		setTimeout(() => {
			focusEditor();
		}, 0);
	}

	function captureShortcutEditorTarget() {
		if (workingDocType !== 'md' || !vditorContainer) return;
		const editorEl = vditorContainer.querySelector('.vditor-ir') as HTMLElement | null;
		const selection = window.getSelection();
		if (!editorEl || !selection || selection.rangeCount === 0) return;
		const range = selection.getRangeAt(0);
		if (!editorEl.contains(range.commonAncestorContainer)) return;
		shortcutEditorRange = range.cloneRange();
		// Capture synchronously, before focusing the textarea changes the browser
		// selection, so write operations retain their cursor/selection target.
		captureEditorSelection();
	}

	function restoreShortcutEditorFocus() {
		if (workingDocType === 'tex') {
			texEditorInstance?.focusEditor?.();
			return;
		}
		if (workingDocType === 'ipynb') {
			ipynbEditorInstance?.focusEditor?.();
			return;
		}
		focusEditor();
		const editorEl = vditorContainer?.querySelector('.vditor-ir') as HTMLElement | null;
		const selection = window.getSelection();
		if (
			editorEl &&
			selection &&
			shortcutEditorRange &&
			editorEl.contains(shortcutEditorRange.commonAncestorContainer)
		) {
			selection.removeAllRanges();
			selection.addRange(shortcutEditorRange);
		}
		shortcutEditorRange = null;
	}

	async function handleChatSidebarShortcut(event: KeyboardEvent) {
		if (
			event.repeat ||
			!shortcutMatches(event, $chatSidebarShortcut) ||
			!note ||
			event.defaultPrevented
		)
			return;
		event.preventDefault();
		event.stopPropagation();

		if ($noteSidebarOpen && document.activeElement === chatTextareaEl) {
			$noteSidebarOpen = false;
			await tick();
			restoreShortcutEditorFocus();
			return;
		}

		captureShortcutEditorTarget();
		activeSidebarTab = 'chat';
		$noteSidebarOpen = true;
		await tick();
		chatTextareaEl?.focus();
	}

	let userScrolledUp = false;

	function handleChatScroll(e: Event) {
		const el = e.currentTarget as HTMLElement;
		const distanceToBottom = el.scrollHeight - el.scrollTop - el.clientHeight;
		userScrolledUp = distanceToBottom > 50;
	}

	function scrollChatToBottom(force = false) {
		if (!chatMessagesEl) return;
		if (force || !userScrolledUp) {
			chatMessagesEl.scrollTop = chatMessagesEl.scrollHeight;
		}
	}

	$effect(() => {
		if (activeSidebarTab !== 'chat') return;
		const chatScrollKey = chatMessages
			.map(
				(msg) =>
					`${msg.role}:${msg.content.length}:${msg.isStreaming ? 1 : 0}:${msg.tools?.length ?? 0}:${msg.error ? 1 : 0}`
			)
			.join('|');
		void chatScrollKey;
		void tick().then(() => {
			scrollChatToBottom();
		});
	});

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
		if (!vditorInstance || !vditorContainer) return null;
		const editorEl = vditorContainer.querySelector('.vditor-ir') as HTMLElement | null;
		if (!editorEl) return null;
		const sel = window.getSelection();
		if (!sel || sel.rangeCount === 0 || sel.isCollapsed) return null;
		const range = sel.getRangeAt(0);
		if (!editorEl.contains(range.commonAncestorContainer)) return null;
		const selText = sel.toString();
		if (!selText.trim()) return null;

		const source = vditorInstance.getValue();
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

	function computeSourceCursor(): AiEditTarget | null {
		if (!vditorInstance || !vditorContainer) return null;
		const editorEl = vditorContainer.querySelector('.vditor-ir') as HTMLElement | null;
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
		const source = vditorInstance.getValue();
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
		armedSelection = null;
	}

	// Keep the captured selection only while the user moves into the prompt.
	// Any other click clears it; a new editor drag captures a fresh selection.
	function onDocMouseDown(e: MouseEvent) {
		const target = e.target as HTMLElement | null;
		if (target?.closest('.prompt-box')) return;
		if (armedSelection) clearArmedSelection();
	}

	function captureEditorSelection() {
		if (!vditorContainer) return;
		const editorEl = vditorContainer.querySelector('.vditor-ir') as HTMLElement | null;
		if (!editorEl) return;
		const sel = window.getSelection();
		if (!sel || sel.rangeCount === 0) return;
		const range = sel.getRangeAt(0);
		if (!editorEl.contains(range.commonAncestorContainer)) return;
		if (sel.isCollapsed) {
			const computed = computeSourceCursor();
			if (computed) {
				armedSelection = { ...computed, chars: 0, words: 0 };
				writeTargetNotice = false;
			}
			return;
		}
		const computed = computeSourceSelection();
		if (computed) {
			const words = computed.text.trim().split(/\s+/).filter(Boolean).length;
			armedSelection = {
				...computed,
				cursor: false,
				chars: computed.text.length,
				words
			};
			writeTargetNotice = false;
		}
	}

	function captureExternalTarget(target: AiEditTarget | null) {
		if (!target) {
			clearArmedSelection();
			return;
		}
		const words = target.text.trim().split(/\s+/).filter(Boolean).length;
		armedSelection = {
			...target,
			chars: target.text.length,
			words
		};
		writeTargetNotice = false;
	}

	// After the AI edits the armed selection, update its source anchors so an
	// immediate follow-up can target the replacement without extra decoration.
	function reselectAfterEdit() {
		if (!armedSelection || !vditorInstance) return;
		const source = vditorInstance.getValue();
		const before = armedSelection.before;
		const after = armedSelection.after;
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
		armedSelection = {
			text: newText,
			before: source.slice(Math.max(0, s - 40), s),
			after: source.slice(e, Math.min(source.length, e + 40)),
			cursor: false,
			chars: newText.length,
			words
		};
	}

	function armedEditTarget(): AiEditTarget | null {
		if (!armedSelection) return null;
		const { chars: _chars, words: _words, ...target } = armedSelection;
		return target;
	}

	function onSelectionChange() {
		clearTimeout(selDebounce);
		selDebounce = setTimeout(captureEditorSelection, 120);
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
		if (!vditorInstance || !vditorContainer) return;
		const editorEl = vditorContainer.querySelector('.vditor-ir') as HTMLElement | null;
		const selection = window.getSelection();
		if (!editorEl || !selection || selection.rangeCount === 0) return;

		const range = selection.getRangeAt(0);
		if (!editorEl.contains(range.commonAncestorContainer)) return;

		savedEditorRange = range.cloneRange();
	}

	function insertAtSavedCursor(linkText: string) {
		if (!vditorInstance || !vditorContainer) return;
		const editorEl = vditorContainer.querySelector('.vditor-ir') as HTMLElement | null;
		if (!editorEl) return;

		focusEditor();

		const selection = window.getSelection();
		if (savedEditorRange && selection) {
			selection.removeAllRanges();
			selection.addRange(savedEditorRange);
		}

		const inserted = document.execCommand('insertText', false, linkText);
		if (!inserted) {
			vditorInstance.insertValue(linkText, true);
		}

		savedEditorRange = null;
		focusEditor();
		draftBody = vditorInstance.getValue();
		triggerAutoSave();
	}

	let mathDialog: HTMLDialogElement | undefined = $state();
	let mathValue = $state('');
	let mathLiveReady = $state(false);
	let katexRenderer = $state<any>(null);
	// Non-empty when the current formula won't render in KaTeX (the engine Vditor
	// uses for $$…$$). Surfaced in the dialog so a bad formula isn't inserted only
	// to silently fail — or render as a red error — later in the note.
	let mathError = $state('');

	function mathToKatex(raw: string): string {
		// MathLive emits \\placeholder tokens KaTeX doesn't know; map them to a box.
		return raw.replace(/\\(?:_)?placeholder(?:\[.*?\])?(?:{})?/g, '\\square');
	}

	async function openMathDialog() {
		try {
			const [{ default: katex }, _mathlive] = await Promise.all([
				import('katex'),
				import('mathlive')
			]);
			katexRenderer = katex;
			mathLiveReady = true;
			mathValue = '';
			mathDialog?.showModal();
		} catch (error) {
			console.error('Failed to load math support', error);
			message = 'Could not load math support.';
		}
	}

	$effect(() => {
		const v = mathValue;
		if (!v.trim()) {
			mathError = '';
			return;
		}
		if (!katexRenderer) {
			mathError = '';
			return;
		}
		try {
			katexRenderer.renderToString(mathToKatex(v), { throwOnError: true, displayMode: true });
			mathError = '';
		} catch (e: any) {
			mathError = e?.message ? String(e.message) : 'KaTeX cannot render this formula.';
		}
	});

	let linkNoteDialog: HTMLDialogElement | undefined = $state();
	let linkSearchQuery = $state('');
	let linkSearchResults = $state<NoteSummary[]>([]);
	let linkSelectedIndex = $state(0);

	let linkDialogMode = $state<'notes' | 'blocks'>('notes');
	let selectedNoteForBlocks = $state<NoteDocument | null>(null);

	type BlockItem = {
		text: string;
		id: string | null;
		original: string;
		isFullNote?: boolean;
		sourceNoteId?: string;
		sourceNoteTitle?: string;
	};
	let allNoteBlocks = $state<BlockItem[]>([]);
	let filteredBlocks = $derived(
		linkDialogMode === 'blocks'
			? linkSearchQuery.trim()
				? allNoteBlocks.filter(
						(b) => b.isFullNote || b.text.toLowerCase().includes(linkSearchQuery.toLowerCase())
					)
				: [...allNoteBlocks]
			: []
	);

	let previewNoteDialog: HTMLDialogElement | undefined = $state();
	let previewNoteTarget = $state<NoteDocument | null>(null);
	let previewNoteContainer: HTMLDivElement | undefined = $state();

	let blockCache: Record<string, string> = {};
	let transclusionObserver: MutationObserver | null = null;

	let toolbarExpanded = $state(false);
	let toolbarNeedsToggle = $state(false);
	let toolbarResizeObserver: ResizeObserver | null = null;

	let saveStatus = $state<'saved' | 'saving' | 'unsaved'>('saved');
	let saveTimer: ReturnType<typeof setTimeout> | null = null;
	let navigationWarningDialog: HTMLDialogElement | undefined = $state();
	let deleteAttachedNoteDialog: HTMLDialogElement | undefined = $state();
	let deleteMainNoteDialog: HTMLDialogElement | undefined = $state();
	let detachPdfDialog: HTMLDialogElement | undefined = $state();

	function requestDeleteMainNote() {
		deleteMainNoteDialog?.showModal();
	}
	let pendingNavigationUrl = $state('');
	let pendingBack = $state(false);

	let attachPdfDialog: HTMLDialogElement | undefined = $state();
	let pdfSearchQuery = $state('');
	let pdfNotesList = $state<NoteDocument[]>([]);
	let pdfSelectedIndex = $state(0);
	let filteredPdfs = $derived(
		pdfSearchQuery.trim()
			? pdfNotesList.filter((p) => p.title.toLowerCase().includes(pdfSearchQuery.toLowerCase()))
			: pdfNotesList
	);
	let shouldRenderEditor = $derived(note !== null && (!isSourceMaterial || showAttachedNote));
	let shouldInitEditor = $derived(note !== null && (!isSourceMaterial || showAttachedNote));
	let loadedRouteNoteId = $state('');

	$effect(() => {
		if (sourceMaterialType === 'pdf' && !PdfViewerComponent) {
			import('$lib/components/PdfViewer.svelte').then(({ default: component }) => {
				PdfViewerComponent = component;
			});
		}
		if (sourceMaterialType === 'epub' && !EpubViewerComponent) {
			import('$lib/components/EpubViewer.svelte').then(({ default: component }) => {
				EpubViewerComponent = component;
			});
		}
		if (sourceMaterialType === 'html' && !HtmlViewerComponent) {
			import('$lib/components/HtmlViewer.svelte').then(({ default: component }) => {
				HtmlViewerComponent = component;
			});
		}
		if (workingDocType === 'tex' && !TexEditorComponent) {
			import('$lib/components/TexEditor.svelte').then(({ default: component }) => {
				TexEditorComponent = component;
			});
		}
		if (workingDocType === 'ipynb' && !IpynbEditorComponent) {
			import('$lib/components/IpynbEditor.svelte').then(({ default: component }) => {
				IpynbEditorComponent = component;
			});
		}
	});

	function appendToNoteBody(content: string) {
		showAttachedNote = true;
		if (vditorInstance) {
			vditorInstance.insertValue(content);
			draftBody = vditorInstance.getValue();
		} else {
			draftBody = `${draftBody}${content}`;
		}
		triggerAutoSave();
	}

	function destroyEditorInstance() {
		if (!vditorInstance) return;
		try {
			vditorInstance.destroy();
		} catch (e) {
			console.warn('Vditor destroy error:', e);
		}
		vditorInstance = null;
	}

	function triggerAutoSave() {
		if (saveStatus !== 'saving') saveStatus = 'unsaved';
		if (saveTimer) clearTimeout(saveTimer);
		saveTimer = setTimeout(() => {
			void saveNote();
		}, 1000);
	}

	function insertMath() {
		if (vditorInstance && mathValue) {
			const cleanMath = mathToKatex(mathValue);
			if (
				mathError &&
				!confirm(`This formula may not render in your note:\n\n${mathError}\n\nInsert it anyway?`)
			) {
				return; // keep the dialog open so the user can fix it
			}
			vditorInstance.insertValue(`\n$$\n${cleanMath}\n$$\n`);
		}
		mathDialog?.close();
	}

	async function pickLatexImage(): Promise<string | null> {
		if (!note) return null;
		const selected = await openFileDialog({
			multiple: false,
			filters: [{ name: 'LaTeX images', extensions: ['png', 'jpg', 'jpeg', 'pdf'] }]
		});
		if (!selected || Array.isArray(selected)) return null;
		try {
			return await invoke<string>('import_latex_asset', {
				noteId: note.id,
				sourcePath: selected
			});
		} catch (error) {
			texCompileError = `Could not import image: ${String(error)}`;
			texPreviewStatus = 'error';
			return null;
		}
	}

	// Compile the open .tex note to PDF and show it in the split preview pane.
	// Shared by the manual button and the debounced auto-compile.
	async function compileTex(opts: { manual?: boolean } = {}) {
		const manual = opts.manual === true;
		if (!note) return;
		if (!manual && !texCacheWarmed) return;
		if (texCompiling) {
			texCompileQueued = true;
			texPreviewStatus = 'pending';
			return;
		}
		texCompiling = true;
		texPreviewStatus = 'compiling';
		texCompileError = null;
		if (manual) isBusy = true;
		try {
			let processedRevision = -1;
			let processedNoteId: string = note.id;
			do {
				texCompileQueued = false;
				const revision = texRevision;
				const noteId: string = note.id;
				processedNoteId = noteId;
				const source = draftBody;
				try {
					if (manual) await saveNote();
					const pdfBytes = await invoke<ArrayBuffer>('compile_latex', { noteId, source });
					// Never let a late compile replace a newer edit or another note's PDF.
					if (note?.id === noteId && revision === texRevision) {
						activeSourceBytes = new Uint8Array(pdfBytes);
						sourceMaterialType = 'pdf';
						showAttachedNote = true;
						texDiagnostics = [];
						texCompileError = null;
						texCacheWarmed = true;
						texPreviewStatus = 'current';
					}
				} catch (e) {
					// Errors from obsolete snapshots are intentionally discarded; the
					// next queued revision will report the relevant result instead.
					if (note?.id === noteId && revision === texRevision) {
						const info = parseLatexError(e);
						texDiagnostics = info.diagnostics;
						texCompileError = info.message;
						texPreviewStatus = 'error';
					}
				}
				processedRevision = revision;
			} while (
				texCompileQueued ||
				(note?.id === processedNoteId && texRevision !== processedRevision)
			);
		} finally {
			texCompiling = false;
			if (texCompileQueued) texPreviewStatus = 'pending';
			if (manual) isBusy = false;
			latexDownloadMsg = null;
		}
	}

	// The backend serialises compile failures as JSON { message, log, diagnostics }
	// (line numbers already mapped to editor coordinates). Fall back to plain text.
	function parseLatexError(e: unknown): {
		message: string;
		diagnostics: { line: number; message: string; severity?: 'error' | 'warning' }[];
	} {
		const raw = typeof e === 'string' ? e : ((e as any)?.message ?? String(e));
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
		activeSourceBytes = null;
		showAttachedNote = false;
	}

	// Debounced auto-compile: a couple of seconds after typing stops, when armed.
	$effect(() => {
		const body = draftBody;
		if (workingDocType === 'tex' && body !== lastTexBody) {
			lastTexBody = body;
			texRevision += 1;
			if (texAutoCompile && texCacheWarmed) texPreviewStatus = 'pending';
		}
		const armed = texAutoCompile && workingDocType === 'tex';
		if (!armed) return;
		if (texAutoTimer) clearTimeout(texAutoTimer);
		texAutoTimer = setTimeout(() => void compileTex(), 350);
		return () => {
			if (texAutoTimer) clearTimeout(texAutoTimer);
		};
	});

	$effect(() => {
		const query = linkSearchQuery;
		if (linkDialogMode === 'notes') {
			if (query.trim()) {
				invoke<SearchResponse>('search_notes', { query }).then((res) => {
					linkSearchResults = res.results.map((r) => r.note);
				});
			} else {
				linkSearchResults = [];
			}
		}
	});

	async function openPreviewModal(noteId: string) {
		isBusy = true;
		try {
			previewNoteTarget = await invoke<NoteDocument>('load_note', { noteId });
			previewNoteDialog?.showModal();
			// Need a tiny delay to ensure previewNoteContainer is bound
			setTimeout(() => {
				if (previewNoteContainer && previewNoteTarget) {
					VditorConstructor?.preview(previewNoteContainer, previewNoteTarget.body, {
						mode: 'dark',
						theme: { current: 'dark' }
					});
				}
			}, 50);
		} catch (err) {
			console.error('Failed to load preview note', err);
			alert('Could not load preview.');
		} finally {
			isBusy = false;
		}
	}

	async function handleVditorClick(e: MouseEvent) {
		const target = e.target as HTMLElement;

		let href = '';

		// 1. Standard HTML links (WYSIWYG or preview modes)
		const link = target.closest('a');
		if (link) {
			href = link.getAttribute('href') || '';
		}

		// 2. Vditor Instant Rendering (IR) mode links
		if (!href) {
			const irLink = target.closest('[data-type="a"]');
			if (irLink) {
				const text = irLink.textContent || '';
				// IR links look like [text](/notes/targetId)
				const match = text.match(/\]\(([^)]+)\)/);
				if (match && match[1]) {
					href = match[1].trim();
				}
			}
		}

		if (!href) return;

		if (href.startsWith('/notes/')) {
			e.preventDefault();
			e.stopPropagation();
			const fullTargetId = decodeURIComponent(href.replace('/notes/', ''));
			const targetId = fullTargetId.split('#')[0];
			await openPreviewModal(targetId);
		}
	}

	function handleVditorKeydownCapture(e: KeyboardEvent) {
		// Prevent WYSIWYG mode shortcut (Cmd/Ctrl + Alt + 7)
		if ((e.ctrlKey || e.metaKey) && e.altKey && !e.shiftKey && e.code === 'Digit7') {
			e.preventDefault();
			e.stopPropagation();
		}

		// Prevent Ctrl+Arrow keys (Up/Down) from scrolling in the editor, but allow Shift for text selection
		if ((e.ctrlKey || e.metaKey) && !e.shiftKey && (e.key === 'ArrowUp' || e.key === 'ArrowDown')) {
			e.preventDefault();
			e.stopPropagation();
		}

		// Vditor has a bug where it freezes during Shift+Arrow selection across nodes.
		// By completely stopping propagation, the browser's native text selection engine
		// takes over flawlessly and Vditor's internal range parser never runs.
		if (e.shiftKey && e.key.startsWith('Arrow')) {
			e.stopImmediatePropagation();
		}
	}

	function handleVditorKeyupCapture(e: KeyboardEvent) {
		// Stop Vditor's keyup processor (which calls expandMarker and freezes)
		if (e.shiftKey && e.key.startsWith('Arrow')) {
			e.stopImmediatePropagation();
		}
	}

	function handleLinkSearchKeydown(e: KeyboardEvent) {
		const targetListLength =
			linkDialogMode === 'notes' ? linkSearchResults.length : filteredBlocks.length;
		if (e.key === 'ArrowDown') {
			e.preventDefault();
			linkSelectedIndex = Math.min(targetListLength - 1, linkSelectedIndex + 1);
		} else if (e.key === 'ArrowUp') {
			e.preventDefault();
			linkSelectedIndex = Math.max(0, linkSelectedIndex - 1);
		} else if (e.key === 'Enter') {
			e.preventDefault();
			if (targetListLength > 0) {
				if (linkDialogMode === 'notes') {
					selectNoteForBlocks(linkSearchResults[linkSelectedIndex]);
				} else {
					insertBlockLink(filteredBlocks[linkSelectedIndex]);
				}
			}
		}
	}

	function autofocus(node: HTMLElement) {
		node.focus();
	}

	function parseBlocks(markdown: string): BlockItem[] {
		const chunks = markdown.split(/\n+/);
		return chunks
			.map((chunk) => {
				const text = chunk.trim();
				if (!text) return null;
				const idMatch = text.match(/\(\(([a-fA-F0-9]{6})\)\)$/);

				let cleanDisplay = text.replace(/\s*\(\([a-fA-F0-9]{6}\)\)$/, '');
				cleanDisplay = cleanDisplay.replace(/\[([^\]]+)\]\([^)]+\)/g, '$1');
				cleanDisplay = cleanDisplay.replace(/(\*\*|__)(.*?)\1/g, '$2');
				cleanDisplay = cleanDisplay.replace(/(\*|_)(.*?)\1/g, '$2');
				cleanDisplay = cleanDisplay.replace(/^#+\s+/g, '');

				return {
					text: cleanDisplay,
					id: idMatch ? idMatch[1] : null,
					original: text
				};
			})
			.filter(Boolean) as BlockItem[];
	}

	async function selectNoteForBlocks(target: NoteSummary) {
		isBusy = true;
		try {
			selectedNoteForBlocks = await invoke<NoteDocument>('load_note', { noteId: target.id });
			allNoteBlocks = [
				{ text: `Link to entire note: ${target.title}`, id: null, original: '', isFullNote: true },
				...parseBlocks(selectedNoteForBlocks.body)
			];
			linkSearchQuery = '';
			linkDialogMode = 'blocks';
			linkSelectedIndex = 0;
		} catch (e) {
			console.error('Failed to load note for blocks', e);
		} finally {
			isBusy = false;
		}
	}

	async function insertBlockLink(block: BlockItem) {
		if (!selectedNoteForBlocks) return;

		if (block.isFullNote) {
			shouldRefocusEditor = true;
			linkNoteDialog?.close();
			const linkText = `[${selectedNoteForBlocks.title}](/notes/${selectedNoteForBlocks.id}) `;
			insertAtSavedCursor(linkText);
			refocusEditorSoon();
			return;
		}

		let blockId = block.id;
		if (!blockId) {
			blockId = Math.random().toString(16).substring(2, 8);
			const newBlockText = `${block.original} ((${blockId}))`;
			selectedNoteForBlocks.body = selectedNoteForBlocks.body.replace(block.original, newBlockText);
			await invoke('save_note', {
				noteId: selectedNoteForBlocks.id,
				title: selectedNoteForBlocks.title,
				tags: selectedNoteForBlocks.tags,
				body: selectedNoteForBlocks.body,
				sourcePdf: selectedNoteForBlocks.sourcePdf,
				annotations: selectedNoteForBlocks.annotations
			});

			if (selectedNoteForBlocks.id === note?.id) {
				setTimeout(() => {
					if (vditorInstance) {
						const editorEl = vditorContainer?.querySelector('.vditor-ir') as HTMLElement | null;
						const selectionOffset = editorEl ? getSelectionTextOffset(editorEl) : null;
						let currentBody = vditorInstance.getValue();
						if (!currentBody.includes(block.original)) {
							const escaped = block.original.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
							const regex = new RegExp(escaped.replace(/\s+/g, '\\s+'));
							currentBody = currentBody.replace(regex, `$& ((${blockId}))`);
						} else {
							currentBody = currentBody.replace(block.original, newBlockText);
						}
						vditorInstance.setValue(currentBody);
						draftBody = currentBody;
						if (selectionOffset !== null) {
							setTimeout(() => {
								focusEditor();
								const refreshedEditorEl = vditorContainer?.querySelector(
									'.vditor-ir'
								) as HTMLElement | null;
								if (refreshedEditorEl)
									restoreSelectionTextOffset(refreshedEditorEl, selectionOffset);
							}, 0);
						}
					}
				}, 50);
			}
		}

		shouldRefocusEditor = true;
		linkNoteDialog?.close();
		const linkText = `[((${blockId}))](/notes/${selectedNoteForBlocks!.id}#${blockId}) `;
		insertAtSavedCursor(linkText);
		refocusEditorSoon();
	}

	let globalSearchDialog: HTMLDialogElement | undefined = $state();
	let globalSearchQuery = $state('');
	let globalSelectedIndex = $state(0);

	let globalBlocks = $state<BlockItem[]>([]);
	let filteredGlobalBlocks = $derived(
		globalSearchQuery.trim()
			? globalBlocks.filter((b) => b.text.toLowerCase().includes(globalSearchQuery.toLowerCase()))
			: globalBlocks.slice(0, 50)
	);

	async function openGlobalBlockSearch() {
		saveCursorPosition();
		globalSearchQuery = '';
		globalSelectedIndex = 0;
		globalSearchDialog?.showModal();
		setTimeout(() => {
			const input = globalSearchDialog?.querySelector('.link-search-input') as HTMLInputElement;
			if (input) input.focus();
		}, 50);

		isBusy = true;
		try {
			const docs = await invoke<NoteDocument[]>('get_all_note_documents');
			const allBlocks: BlockItem[] = [];
			for (const doc of docs) {
				const blocks = parseBlocks(doc.body);
				for (const b of blocks) {
					b.sourceNoteId = doc.id;
					b.sourceNoteTitle = doc.title;
					allBlocks.push(b);
				}
			}
			globalBlocks = allBlocks;
		} catch (err) {
			console.error('Failed to load global blocks', err);
		} finally {
			isBusy = false;
		}
	}

	function handleGlobalSearchKeydown(e: KeyboardEvent) {
		const targetListLength = filteredGlobalBlocks.length;
		if (e.key === 'ArrowDown') {
			e.preventDefault();
			globalSelectedIndex = Math.min(targetListLength - 1, globalSelectedIndex + 1);
		} else if (e.key === 'ArrowUp') {
			e.preventDefault();
			globalSelectedIndex = Math.max(0, globalSelectedIndex - 1);
		} else if (e.key === 'Enter') {
			e.preventDefault();
			if (targetListLength > 0) {
				void insertGlobalBlockLink(filteredGlobalBlocks[globalSelectedIndex]);
			}
		}
	}

	async function insertGlobalBlockLink(block: BlockItem) {
		if (!block.sourceNoteId || !block.sourceNoteTitle) return;

		let blockId = block.id;
		const isNewBlock = !blockId;
		if (isNewBlock) {
			blockId = Math.random().toString(16).substring(2, 8);
		}

		shouldRefocusEditor = true;
		globalSearchDialog?.close();
		const linkText = `[((${blockId}))](/notes/${block.sourceNoteId}#${blockId}) `;

		if (isNewBlock) {
			const newBlockText = `${block.original} ((${blockId}))`;
			isBusy = true;
			try {
				const sourceDoc = await invoke<NoteDocument>('load_note', { noteId: block.sourceNoteId });
				sourceDoc.body = sourceDoc.body.replace(block.original, newBlockText);
				await invoke('save_note', {
					noteId: sourceDoc.id,
					title: sourceDoc.title,
					tags: sourceDoc.tags,
					body: sourceDoc.body,
					sourcePdf: sourceDoc.sourcePdf,
					annotations: sourceDoc.annotations
				});

				if (sourceDoc.id === note?.id) {
					setTimeout(() => {
						if (vditorInstance) {
							const editorEl = vditorContainer?.querySelector('.vditor-ir') as HTMLElement | null;
							const selectionOffset = editorEl ? getSelectionTextOffset(editorEl) : null;
							let currentBody = vditorInstance.getValue();
							if (!currentBody.includes(block.original)) {
								const escaped = block.original.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
								const regex = new RegExp(escaped.replace(/\s+/g, '\\s+'));
								currentBody = currentBody.replace(regex, `$& ((${blockId}))`);
							} else {
								currentBody = currentBody.replace(block.original, newBlockText);
							}
							vditorInstance.setValue(currentBody);
							draftBody = currentBody;
							if (selectionOffset !== null) {
								setTimeout(() => {
									focusEditor();
									const refreshedEditorEl = vditorContainer?.querySelector(
										'.vditor-ir'
									) as HTMLElement | null;
									if (refreshedEditorEl)
										restoreSelectionTextOffset(refreshedEditorEl, selectionOffset);
								}, 0);
							}
						}
					}, 50);
				}

				insertAtSavedCursor(linkText);
			} catch (err) {
				console.error('Failed to append block ID to source note', err);
				message = 'Failed to update source note.';
				setTimeout(() => (message = ''), 3000);
			} finally {
				isBusy = false;
				refocusEditorSoon();
			}
		} else {
			insertAtSavedCursor(linkText);
			refocusEditorSoon();
		}
	}

	async function loadCurrentNote(noteId: string) {
		isLoadingNote = true;
		toolsReady = false;
		clearArmedSelection();
		writeTargetNotice = false;
		if (chatPersistTimer) {
			clearTimeout(chatPersistTimer);
			chatPersistTimer = undefined;
		}
		const previousAiNoteId = activeAiNoteId();
		if (previousAiNoteId && previousAiNoteId !== noteId && chatMessages.length) {
			await persistChatHistory(previousAiNoteId, chatMessages);
		}
		destroyEditorInstance();
		activeSourceBytes = null;
		activeSourceId = null;
		showAttachedNote = false;
		note = null;

		try {
			note = await invoke<NoteDocument>('load_note', { noteId });
			const loadedNote = note;
			// Keep the persisted transcript untouched; reasoning is removed only from
			// the assistant's presentation below.
			chatMessages = loadedNote.chatHistory || [];
			noteHistory = [];
			versionPreviewContent = null;
			activeSidebarTab = 'info';

			const relLower = loadedNote.relativePath.toLowerCase();
			isSourceMaterial =
				relLower.endsWith('.pdf') || relLower.endsWith('.epub') || relLower.endsWith('.html');

			if (isSourceMaterial) {
				sourceMaterialType = relLower.endsWith('.pdf')
					? 'pdf'
					: relLower.endsWith('.epub')
						? 'epub'
						: 'html';
				workingDocType = 'md';

				const allNotes = await invoke<NoteDocument[]>('get_all_note_documents');
				const existingScratchpad =
					allNotes
						.filter((candidate) => candidate.sourcePdf === loadedNote.id)
						.sort((left, right) => right.updatedAt.localeCompare(left.updatedAt))[0] ?? null;
				draftTitle = loadedNote.title;
				draftBody = existingScratchpad?.body ?? '';
				draftTags = loadedNote.tags.join(', ');
				activeSourceId = loadedNote.id;
				const bytes = await invoke<ArrayBuffer>('read_pdf_binary', { noteId: loadedNote.id });
				activeSourceBytes = new Uint8Array(bytes);
				scratchpadSavedId = existingScratchpad?.id ?? null;
				// Opening the source document should show only that document. The
				// linked note has its own dashboard row and opens in split view from
				// there; it remains available here through the Attach Note button.
				showAttachedNote = false;
				noteOpened(loadedNote.id, 'chat');
			} else {
				workingDocType = relLower.endsWith('.tex')
					? 'tex'
					: relLower.endsWith('.ipynb')
						? 'ipynb'
						: 'md';

				draftTitle = loadedNote.title;
				draftBody = loadedNote.body;
				draftTags = loadedNote.tags.join(', ');

				if (loadedNote.sourcePdf) {
					activeSourceId = loadedNote.sourcePdf;
					const bytes = await invoke<ArrayBuffer>('read_pdf_binary', {
						noteId: loadedNote.sourcePdf
					});
					activeSourceBytes = new Uint8Array(bytes);
					// This route was opened through the note itself, so keep its
					// editor visible even when the note is still empty.
					showAttachedNote = true;
					scratchpadSavedId = loadedNote.id;
					noteOpened(loadedNote.id, 'chat');
					// If a working document has a sourcePdf, we need to know its type.
					// We'll query it or assume it's PDF for now unless we know otherwise.
					// (We can load it to find out)
					try {
						const sourceDoc = await invoke<NoteDocument>('load_note', {
							noteId: loadedNote.sourcePdf
						});
						const sRel = sourceDoc.relativePath.toLowerCase();
						sourceMaterialType = sRel.endsWith('.pdf')
							? 'pdf'
							: sRel.endsWith('.epub')
								? 'epub'
								: 'html';
					} catch (e) {
						sourceMaterialType = 'pdf'; // fallback
					}
				} else {
					activeSourceId = null;
					activeSourceBytes = null;
					sourceMaterialType = null;
					showAttachedNote = true;
					scratchpadSavedId = null;
					noteOpened(loadedNote.id, 'chat');
				}
			}

			message = '';
			void fetchRelatedNotes();
		} catch (error) {
			console.error('Failed to open note', error);
			message = 'Could not open this note.';
		} finally {
			isLoadingNote = false;
			// Let the note and its surrounding layout paint before requesting the
			// comparatively heavy editor/tool bundle.
			if (note) {
				await tick();
				await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
				toolsReady = true;
			}
		}
	}

	async function refreshCurrentNoteFromBackend(skipEditorUpdate = false) {
		if (!note) return;
		const refreshed = await invoke<NoteDocument>('load_note', { noteId: note.id });
		note = {
			...refreshed,
			chatHistory: chatMessages
		};
		if (!isSourceMaterial && workingDocType === 'md') {
			draftTitle = refreshed.title;
			draftBody = refreshed.body;
			draftTags = refreshed.tags.join(', ');
			if (!skipEditorUpdate && vditorInstance && vditorInstance.getValue() !== refreshed.body) {
				vditorInstance.setValue(refreshed.body);
			}
		} else if (!isSourceMaterial) {
			draftTitle = refreshed.title;
			draftBody = refreshed.body;
			draftTags = refreshed.tags.join(', ');
		}
		void fetchRelatedNotes();
	}

	// A live note stream is starting (whole-body replace). Keep the existing note
	// visible until the first real content arrives; clearing here made fast tool
	// calls flash an empty editor before the authoritative write landed.
	function beginNoteStream() {
		noteStreamBackup = vditorInstance ? vditorInstance.getValue() : draftBody;
		noteStreamBuf = '';
		noteStreaming = true;
		// Every stream flush rebuilds the whole IR DOM; keep the transclusion
		// observer disconnected until the stream settles so it doesn't drain
		// full-tree mutation batches each frame. scanForTransclusions runs once
		// after the stream lands (setupTransclusionObserver re-arms it).
		if (transclusionObserver) transclusionObserver.disconnect();
		// Locate the cursor/selection target once: it is stable for the whole
		// request, so per-delta re-scanning a large note is wasted work.
		noteStreamSpan = activeAiEditTarget
			? locateNoteStreamTarget(noteStreamBackup, activeAiEditTarget)
			: null;
		scheduleNoteStreamFlush();
	}

	function scheduleNoteStreamFlush() {
		if (noteStreamFlushPending) return;
		noteStreamFlushPending = true;
		requestAnimationFrame(() => {
			noteStreamFlushPending = false;
			flushNoteStream();
		});
	}

	// One editor rebuild per frame, coalescing all deltas that arrived since the
	// last flush. Restores the caret/selection across the rebuild so streaming
	// no longer destroys the user's cursor position every token.
	function flushNoteStream() {
		if (!noteStreaming) return;
		const result = composeNoteStreamPreviewWithStatus(
			noteStreamBackup,
			noteStreamBuf,
			activeAiEditTarget,
			noteStreamSpan
		);
		if (vditorInstance) {
			// getSelectionTextOffset walks every text node of the editor; only do
			// it when the editor actually has focus and a live selection. The
			// user isn't interacting mid-stream, so skip the walk otherwise.
			const editorEl = vditorContainer?.querySelector('.vditor-ir') as HTMLElement | null;
			const hasSelection =
				editorEl?.contains(document.activeElement) && window.getSelection()?.rangeCount !== 0;
			const selectionOffset = editorEl && hasSelection ? getSelectionTextOffset(editorEl) : null;
			vditorInstance.setValue(result.preview);
			if (selectionOffset !== null) {
				const refreshed = vditorContainer?.querySelector('.vditor-ir') as HTMLElement | null;
				if (refreshed) {
					restoreSelectionTextOffset(refreshed, Math.min(selectionOffset, result.preview.length));
				}
			}
			// Keep draftBody in sync with the live preview so any mid-stream save
			// (title/tag autosave, exit) carries the streamed content instead of
			// racing the backend with a stale body.
			draftBody = result.preview;
		}
	}

	// A token (or several) of the note arrived — buffer it and coalesce the
	// editor update to the next animation frame.
	function appendNoteStream(delta: string): boolean {
		if (!noteStreaming) beginNoteStream();
		noteStreamBuf += delta;
		scheduleNoteStreamFlush();
		return noteStreamSpan !== null || !activeAiEditTarget;
	}

	// The stream turned out not to be a whole-body replace (append/edit) — undo
	// the live preview; the authoritative note_written will apply the real change.
	function cancelNoteStream() {
		if (!noteStreaming) return;
		noteStreaming = false;
		if (vditorInstance) vditorInstance.setValue(noteStreamBackup);
		setupTransclusionObserver();
	}

	// Authoritative result of a write_note tool call. Sets the final content in
	// one shot (no fake animation) and reconciles any live-streamed preview.
	function applyNoteWrite(newContent: string, mode: 'write' | 'append') {
		noteStreaming = false;
		// Append needs the CURRENT note as its base on every editor. Markdown uses
		// the live vditor value; .tex/.ipynb editors (CodeMirror/cell UI) track the
		// same text in draftBody. Basing append on '' for non-md editors would set
		// the note to just the appended fragment — and the editor's change listener
		// would then autosave that truncated body to disk.
		const baseContent =
			mode === 'append'
				? (vditorInstance ? vditorInstance.getValue() : draftBody).trimEnd() + '\n\n'
				: '';
		const finalContent = baseContent + newContent;
		if (note) note = { ...note, body: finalContent };
		draftBody = finalContent;
		// Avoid a second visible reset only when the editor itself already contains
		// the authoritative result. The streamed buffer may be stale or may cover
		// only a cursor/selection target. clearStack resets Vditor's undo history so
		// Ctrl+Z doesn't walk back through every mid-stream snapshot.
		if (vditorInstance && editorNeedsAuthoritativeBody(vditorInstance.getValue(), finalContent)) {
			vditorInstance.setValue(finalContent, true);
		}
		// Re-arm the transclusion observer disconnected during streaming and scan
		// once so the settled content picks up any new links.
		setupTransclusionObserver();
	}

	function initVditor() {
		if (!VditorConstructor || !vditorContainer || vditorInstance) return;

		try {
			vditorInstance = new VditorConstructor(vditorContainer, {
				value: draftBody,
				placeholder: isSourceMaterial ? 'Scratchpad for notes...' : 'Start typing here...',
				mode: 'ir',
				// Vditor ships its own skin; 'classic' is its light theme. We mirror the
				// app theme here and keep it in sync via the $effect below. Pass only the
				// skin (no content/code theme) so Vditor doesn't fetch theme CSS from a CDN
				// — the editor's bg/text colors come from our own var overrides anyway.
				theme: $theme === 'light' ? 'classic' : 'dark',
				icon: 'material',
				lang: 'en_US',
				tab: '\t',
				cache: { enable: false },
				toolbarConfig: { pin: true },
				toolbar: [
					{
						name: 'attach-pdf',
						tipPosition: 'n',
						tip: 'Attach PDF',
						icon: '<svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"></path><polyline points="14 2 14 8 20 8"></polyline></svg>',
						click: () => {
							openAttachPdfDialog();
						}
					},
					'|',
					'emoji',
					'headings',
					'bold',
					'italic',
					'strike',
					'link',
					'|',
					'list',
					'ordered-list',
					'check',
					'outdent',
					'indent',
					'|',
					'quote',
					'line',
					'code',
					'inline-code',
					'insert-before',
					'insert-after',
					'|',
					{
						name: 'mathlive',
						tipPosition: 'n',
						tip: 'MathLive Editor',
						icon: '<svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round"><path d="M18 4H6l6 8-6 8h12"></path></svg>',
						click: () => {
							void openMathDialog();
						}
					},
					{
						name: 'link-note',
						tipPosition: 'n',
						tip: 'Link to Note',
						icon: '<svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round"><path d="M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71"></path><path d="M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71"></path></svg>',
						click: () => {
							saveCursorPosition();
							linkSearchQuery = '';
							linkSearchResults = [];
							linkNoteDialog?.showModal();
							setTimeout(() => {
								const input = linkNoteDialog?.querySelector(
									'.link-search-input'
								) as HTMLInputElement;
								if (input) input.focus();
							}, 50);
						}
					},
					{
						name: 'search-blocks',
						tipPosition: 'n',
						tip: 'Search Global Blocks',
						icon: '<svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round"><circle cx="11" cy="11" r="8"></circle><line x1="21" y1="21" x2="16.65" y2="16.65"></line></svg>',
						click: () => {
							openGlobalBlockSearch();
						}
					},
					'|',
					'upload',
					'record',
					'table',
					'|',
					'undo',
					'redo',
					'|',
					'fullscreen',
					'edit-mode',
					{
						name: 'more',
						toolbar: ['both', 'code-theme', 'content-theme', 'outline', 'devtools', 'info', 'help']
					}
				],
				after: () => {
					const toolbar = vditorContainer?.querySelector('.vditor-toolbar');
					if (toolbar) {
						toolbarResizeObserver = new ResizeObserver(() => {
							if (toolbar.scrollHeight > 55) {
								toolbarNeedsToggle = true;
							} else {
								toolbarNeedsToggle = false;
								toolbarExpanded = false;
							}
							updateToolbarOverflow();
						});
						toolbarResizeObserver.observe(toolbar);
						if (toolbar.scrollHeight > 55) {
							toolbarNeedsToggle = true;
						}
						updateToolbarOverflow();

						const fsBtn = toolbar.querySelector('button[data-type="fullscreen"]');
						if (fsBtn) {
							const label = fsBtn.getAttribute('aria-label') || '';
							const match = label.match(/<([^>]+)>/);
							if (match) {
								fullscreenShortcut = match[1];
							}
						}
					}
					setTimeout(() => {
						scanForTransclusions();
					}, 100);
					setupTransclusionObserver();
				},
				keydown: (e: KeyboardEvent) => {
					if ((e.ctrlKey || e.metaKey) && e.code === 'Comma') {
						e.preventDefault();
						if (e.shiftKey) {
							const globalSearchBtn = vditorContainer?.querySelector(
								'button[data-type="search-blocks"]'
							) as HTMLButtonElement | null;
							if (globalSearchBtn) globalSearchBtn.click();
						} else {
							const linkBtn = vditorContainer?.querySelector(
								'button[data-type="link-note"]'
							) as HTMLButtonElement | null;
							if (linkBtn) linkBtn.click();
						}
						return;
					}
					if ((e.ctrlKey || e.metaKey) && e.shiftKey && e.key.toLowerCase() === 'z') {
						e.preventDefault();
						const redoBtn = vditorContainer?.querySelector(
							'button[data-type="redo"]'
						) as HTMLButtonElement | null;
						if (redoBtn) redoBtn.click();
					}
				},
				input: (value: string) => {
					draftBody = value;
					triggerAutoSave();
				}
			});
		} catch (e: any) {
			message = 'Vditor Error: ' + (e?.message || String(e));
		}
	}

	$effect(() => {
		if (!toolsReady || !shouldInitEditor || !vditorContainer || vditorInstance) return;
		if (!VditorConstructor && !vditorLoading) {
			vditorLoading = true;
			Promise.all([import('vditor'), import('vditor/dist/index.css')])
				.then(([{ default: component }]) => {
					VditorConstructor = component;
					vditorLoading = false;
					initVditor();
				})
				.catch((error) => {
					vditorLoading = false;
					message = 'Could not load the Markdown editor.';
					console.error('Failed to load Vditor', error);
				});
		} else if (VditorConstructor) {
			initVditor();
		}
	});

	// Keep Vditor's skin in sync when the app theme is toggled while a note is open.
	$effect(() => {
		const skin = $theme === 'light' ? 'classic' : 'dark';
		if (vditorInstance) vditorInstance.setTheme(skin);
	});

	function parseBacklinkContext(context: string): string {
		if (!context) return '';
		let html = context;
		// Strip markdown links but keep text and make it look like a link
		html = html.replace(
			/\[([^\]]+)\]\([^)]+\)/g,
			'<span style="color: var(--accent-200); font-weight: 500;">$1</span>'
		);
		// Strip transclusion syntax
		html = html.replace(
			/\(\([a-fA-F0-9]{6}\)\)/g,
			'<span style="color: var(--text-secondary);">(Block Link)</span>'
		);
		// Bold and italic
		html = html.replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>');
		html = html.replace(/\*([^*]+)\*/g, '<em>$1</em>');
		return html;
	}

	function scanForTransclusions() {
		if (!vditorContainer) return;
		const links = vditorContainer.querySelectorAll('[data-type="a"]:not(.transclusion-wrapper)');
		links.forEach((linkWrapper) => {
			const irLink = linkWrapper.querySelector('.vditor-ir__link');
			if (!irLink) return;
			const text = irLink.textContent || '';
			const blockMatch = text.match(/^\(\(([a-fA-F0-9]{6})\)\)$/);
			if (!blockMatch) return;

			const blockId = blockMatch[1];
			const fullText = linkWrapper.textContent || '';
			const urlMatch = fullText.match(/\]\(\/notes\/([^#]+)#([a-fA-F0-9]{6})\)$/);
			if (!urlMatch) return;

			const targetNoteId = urlMatch[1];
			linkWrapper.classList.add('transclusion-wrapper');

			// Load block content for the tooltip and CSS rendering — no DOM injection
			const cacheKey = `${targetNoteId}#${blockId}`;
			if (blockCache[cacheKey]) {
				const plainText = blockCache[cacheKey].replace(/<[^>]+>/g, '');
				(linkWrapper as HTMLElement).title = plainText;
				(linkWrapper as HTMLElement).setAttribute('data-block-content', plainText);
			} else {
				invoke<NoteDocument>('load_note', { noteId: targetNoteId })
					.then((n) => {
						const blocks = parseBlocks(n.body);
						const targetBlock = blocks.find((b) => b.id === blockId);
						if (targetBlock) {
							const rawMd = targetBlock.original.replace(/\s*\(\([a-fA-F0-9]+\)\)$/, '').trim();
							let htmlText = rawMd;
							htmlText = htmlText.replace(
								/\[([^\]]+)\]\(([^)]+)\)/g,
								'<span class="mock-link">$1</span>'
							);
							htmlText = htmlText.replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>');
							htmlText = htmlText.replace(/\*([^*]+)\*/g, '<em>$1</em>');
							blockCache[cacheKey] = htmlText;
							// Set plain-text tooltip and data attribute
							const plainText = htmlText.replace(/<[^>]+>/g, '');
							(linkWrapper as HTMLElement).title = plainText;
							(linkWrapper as HTMLElement).setAttribute('data-block-content', plainText);
						}
					})
					.catch(() => {});
			}
		});
	}

	function setupTransclusionObserver() {
		if (!vditorContainer) return;
		if (transclusionObserver) transclusionObserver.disconnect();

		transclusionObserver = new MutationObserver(() => {
			// Streaming setValue rebuilds the entire IR DOM every frame; the
			// observer is disconnected for the whole stream (see beginNoteStream)
			// and re-armed when it settles, so scanning only runs on real edits.
			scanForTransclusions();
		});

		transclusionObserver.observe(vditorContainer, {
			childList: true,
			subtree: true,
			characterData: true
		});
	}

	async function fetchRelatedNotes() {
		if (!draftTags.trim()) {
			relatedNotes = [];
			return;
		}
		try {
			const query = draftTags.split(',')[0].trim();
			if (query) {
				const res = await invoke<SearchResponse>('search_notes', { query });
				relatedNotes = res.results
					.map((r) => r.note)
					.filter((n) => n.id !== note?.id)
					.slice(0, 5);
			}
		} catch (e) {
			console.error(e);
		}
	}

	function handleAnnotationsChange(anns: PdfAnnotation[]) {
		if (note) {
			note.annotations = anns;
			triggerAutoSave();
		}
	}

	function handleImageExtract(base64: string) {
		appendToNoteBody(`\n\n![Extracted Image](${base64})\n\n`);
	}

	function handlePdfTextExtracted(text: string) {
		if (!activeSourceId || !note) return;
		const sourceId = activeSourceId;
		const sourceTitle = isSourceMaterial ? note.title : `${draftTitle} — attached PDF`;
		pdfIngestionStatus = 'indexing';
		pdfIngestionError = null;
		const startedAt = Date.now();
		const startEntry: DebugTraceEntry = {
			time: startedAt,
			kind: 'config',
			msg: `PDF indexing started: ${sourceTitle} (${sourceId}), ${text.length.toLocaleString()} extracted characters`
		};
		pendingDebugTrace = [...pendingDebugTrace, startEntry];
		pdfIngestionPromise = (async () => {
			try {
				const result = await invoke<{ status: 'cached' | 'indexed' | 'empty'; chunks: number }>(
					'ensure_document_ingested',
					{ docId: sourceId, source: sourceTitle, text }
				);
				const entry: DebugTraceEntry = {
					time: Date.now(),
					kind: 'done',
					msg: `PDF indexing ${result.status}: ${sourceTitle} (${result.chunks} chunks)`
				};
				pendingDebugTrace = [...pendingDebugTrace, entry];
				if (debugInfo) debugInfo = { ...debugInfo, trace: [...debugInfo.trace, startEntry, entry] };
				if (activeSourceId === sourceId) pdfIngestionStatus = result.status;
			} catch (error) {
				console.error('Failed to index PDF text', error);
				const detail =
					typeof error === 'string'
						? error
						: error instanceof Error
							? error.message
							: JSON.stringify(error) || String(error);
				const entry: DebugTraceEntry = {
					time: Date.now(),
					kind: 'error',
					msg: `PDF indexing failed for ${sourceTitle} (${sourceId}): ${detail}`
				};
				pendingDebugTrace = [...pendingDebugTrace, entry];
				showDebugWindow = true;
				debugInfo = debugInfo
					? { ...debugInfo, trace: [...debugInfo.trace, startEntry, entry] }
					: {
							requestStart: startedAt,
							firstChunk: null,
							generationStart: null,
							generationEnd: null,
							done: entry.time,
							promptTokens: 0,
							completionTokens: 0,
							totalTokens: 0,
							turnCount: 0,
							replyChars: 0,
							trace: [startEntry, entry]
						};
				if (activeSourceId === sourceId) {
					pdfIngestionStatus = 'failed';
					pdfIngestionError = detail;
				}
			}
		})();
	}

	async function saveNote() {
		if (!note) return;
		isBusy = true;
		saveStatus = 'saving';
		try {
			let targetId = note.id;
			if (isSourceMaterial) {
				if (!scratchpadSavedId) {
					const newNote = await invoke<NoteDocument>('create_note', {
						title: draftTitle,
						sourcePdf: activeSourceId,
						notebook: openNoteNotebook()
					});
					scratchpadSavedId = newNote.id;
				}
				targetId = scratchpadSavedId;
			}

			const sentTitle = draftTitle;
			const saved = await invoke<NoteDocument>('save_note', {
				noteId: targetId,
				title: sentTitle,
				tags: draftTags
					.split(',')
					.map((tag) => tag.trim())
					.filter(Boolean),
				body: draftBody,
				sourcePdf: activeSourceId,
				// For Source Material main notes, annotations belong to the source note, not the scratchpad
				annotations: isSourceMaterial ? [] : note.annotations
			});

			if (isSourceMaterial && note.annotations.length > 0) {
				await invoke('save_pdf_annotations', { noteId: note.id, annotations: note.annotations });
			}

			if (!isSourceMaterial) {
				note = saved;
			}

			if (draftTitle === sentTitle) {
				draftTitle = saved.title;
			}

			saveStatus = 'saved';
			void fetchRelatedNotes();
			if (activeSidebarTab === 'versions') {
				void fetchNoteHistory();
			}
		} catch (err) {
			console.error('Save error:', err);
			saveStatus = 'unsaved';
			message = `Save failed: ${err}`;
		} finally {
			isBusy = false;
		}
	}

	async function deleteNote() {
		if (!note) return;
		isBusy = true;
		try {
			await invoke('delete_note', { noteId: note.id });
			await goto(resolve('/'));
		} finally {
			isBusy = false;
		}
	}

	async function duplicateNote() {
		if (!note) return;
		isBusy = true;
		try {
			const duplicated = await invoke<NoteDocument>('duplicate_note', { noteId: note.id });
			// Navigate and reload
			safeNavigate(`/notes/${encodeURIComponent(duplicated.id)}`);
		} finally {
			isBusy = false;
		}
	}

	async function stopActiveChat(): Promise<boolean> {
		if (!activeChatRequestId && !isChatStreaming) return true;
		try {
			await invoke('cancel_ai');
		} catch (error) {
			console.error('Failed to stop AI:', error);
			return false;
		}

		// cancel_ai is cooperative: wait until the backend emits done/error and
		// clears the active request before retrying or restoring a snapshot.
		const deadline = Date.now() + 10_000;
		while ((activeChatRequestId || isChatStreaming) && Date.now() < deadline) {
			await new Promise((resolve) => setTimeout(resolve, 50));
		}
		if (activeChatRequestId || isChatStreaming) {
			console.error('AI request did not stop within 10 seconds');
			return false;
		}
		return true;
	}

	function stopChat() {
		if (!isChatStreaming && !activeChatRequestId) return;
		void stopActiveChat();
	}

	function beginAiRequest(
		requestId: string,
		composerMode: 'chat' | 'editor',
		statusText: string,
		aiNoteId: string
	): number {
		const startTime = Date.now();
		pendingDebugTrace = [
			{ time: startTime, msg: 'Request sent', kind: 'send' },
			{ time: startTime, msg: `Composer mode: ${composerMode}`, kind: 'config' }
		];
		debugInfo = showDebugWindow
			? {
					requestStart: startTime,
					firstChunk: null,
					generationStart: null,
					generationEnd: null,
					done: null,
					promptTokens: 0,
					completionTokens: 0,
					totalTokens: 0,
					turnCount: 0,
					replyChars: 0,
					trace: pendingDebugTrace
				}
			: null;
		chatMessages = [
			...chatMessages,
			{ role: 'assistant', content: '', isStreaming: true, startTime, statusText }
		];
		activeAiComposerMode = composerMode;
		activeChatRequestId = requestId;
		activeChatNoteId = aiNoteId;
		setTimeout(() => scrollChatToBottom(true), 50);
		return startTime;
	}

	async function sendChatMessage() {
		if (!note || !chatInput.trim() || isChatStreaming) return;
		if (aiInteractionMode === 'operation' && !armedEditTarget()) {
			writeTargetNotice = true;
			return;
		}
		const userText = chatInput.trim();
		chatInput = '';
		if (chatTextareaEl) chatTextareaEl.style.height = 'auto';
		await sendChatText(userText);
	}

	async function sendChatText(userText: string) {
		if (!note) return;
		const editorTarget = armedEditTarget();
		if (aiInteractionMode === 'operation' && !editorTarget) {
			writeTargetNotice = true;
			if (!chatInput.trim()) chatInput = userText;
			await tick();
			if (chatTextareaEl) {
				chatTextareaEl.style.height = 'auto';
				chatTextareaEl.style.height = `${Math.min(chatTextareaEl.scrollHeight + 2, 150)}px`;
				chatTextareaEl.focus();
			}
			return;
		}
		if ((isSourceMaterial && showAttachedNote) || saveStatus !== 'saved') {
			await saveNote();
		}
		if (pdfIngestionPromise) await pdfIngestionPromise;
		const aiNoteId = activeAiNoteId();
		if (!aiNoteId) {
			message = 'Open or create the attached note before asking the AI to edit it.';
			return;
		}
		const requestId = Date.now().toString();
		const composerMode = aiInteractionMode === 'operation' ? 'editor' : 'chat';
		const selection =
			composerMode === 'editor' ? editorTarget : editorTarget?.cursor ? null : editorTarget;
		activeAiEditTarget = composerMode === 'editor' ? selection : null;
		const snapshot: NoteSnapshot = {
			noteBody: draftBody,
			draftTitle: draftTitle,
			draftTags: draftTags,
			chatLength: chatMessages.length
		};
		chatMessages = [
			...chatMessages,
			{ role: 'user', content: userText, snapshotId: requestId, snapshot }
		];
		beginAiRequest(
			requestId,
			composerMode,
			composerMode === 'editor' ? 'Preparing note edit…' : 'Retrieving note and PDF context…',
			aiNoteId
		);
		checkpointChatHistory(0);
		try {
			await invoke('ask_ai_stream', {
				noteId: aiNoteId,
				question: userText,
				requestId,
				// Working-doc type so the model edits as LaTeX / notebook, not Markdown.
				docType: workingDocType,
				selection,
				interactionMode: aiInteractionMode
			});
		} catch (e) {
			console.error('AI Error:', e);
			failStreamingChatMessage(requestId, extractChatErrorMessage(e));
		}
	}

	async function rewindToSnapshot(snapshot?: NoteSnapshot, fillInput?: string) {
		if (!snapshot || !note) return;
		const aiNoteId = activeAiNoteId();
		if (!aiNoteId) return;
		if (!(await stopActiveChat())) return;
		chatMessages = chatMessages.slice(0, snapshot.chatLength);
		draftBody = snapshot.noteBody;
		draftTitle = snapshot.draftTitle;
		draftTags = snapshot.draftTags;
		if (note) note = { ...note, body: snapshot.noteBody, title: snapshot.draftTitle };
		if (vditorInstance) vditorInstance.setValue(snapshot.noteBody);
		if (fillInput !== undefined) {
			chatInput = fillInput;
			await tick();
			if (chatTextareaEl) {
				chatTextareaEl.style.height = 'auto';
				chatTextareaEl.style.height = `${Math.min(chatTextareaEl.scrollHeight, 200)}px`;
				chatTextareaEl.focus();
			}
		}

		isBusy = true;
		try {
			await invoke('save_note', {
				noteId: aiNoteId,
				title: snapshot.draftTitle,
				tags: snapshot.draftTags
					.split(',')
					.map((t: string) => t.trim())
					.filter(Boolean),
				body: snapshot.noteBody,
				sourcePdf: activeSourceId,
				annotations: isSourceMaterial ? [] : note.annotations
			});
			await invoke('save_chat_history', { noteId: aiNoteId, chatHistory: chatMessages });
			// The backend conversation includes tool calls/results that are not
			// represented in the UI history. Clear it after a rewind so the next
			// retry rebuilds from the newly persisted authoritative history.
			await invoke('clear_ai_conversation', { noteId: aiNoteId });
		} catch (err) {
			console.error('Failed to rewind:', err);
		} finally {
			isBusy = false;
		}
	}

	async function retryMessage(snapshot: NoteSnapshot, userText: string) {
		await rewindToSnapshot(snapshot);
		await sendChatText(userText);
	}

	function mergeChatTools(
		existing: { name: string; details: string }[] = [],
		incoming: { name: string; details: string }[] = []
	) {
		const merged = [...existing];
		for (const tool of incoming) {
			if (!merged.some((entry) => entry.name === tool.name && entry.details === tool.details)) {
				merged.push(tool);
			}
		}
		return merged;
	}

	async function reconcileRequestNote(expectedNoteId: string) {
		if (!canApplyReconciledNote(expectedNoteId, activeAiNoteId())) return;
		const refreshed = await invoke<NoteDocument>('load_note', { noteId: expectedNoteId });
		// Loading is asynchronous. Re-check after it completes so navigation during
		// the request cannot let a late completion overwrite the newly opened note.
		if (!canApplyReconciledNote(expectedNoteId, activeAiNoteId())) return;
		if (!isSourceMaterial) {
			note = { ...refreshed, chatHistory: chatMessages };
			draftTitle = refreshed.title;
			draftBody = refreshed.body;
			draftTags = refreshed.tags.join(', ');
			if (
				workingDocType === 'md' &&
				vditorInstance &&
				editorNeedsAuthoritativeBody(vditorInstance.getValue(), refreshed.body)
			) {
				vditorInstance.setValue(refreshed.body);
			}
		} else {
			draftTitle = refreshed.title;
			draftBody = refreshed.body;
			draftTags = refreshed.tags.join(', ');
			if (
				vditorInstance &&
				editorNeedsAuthoritativeBody(vditorInstance.getValue(), refreshed.body)
			) {
				vditorInstance.setValue(refreshed.body);
			}
		}
		void fetchRelatedNotes();
	}

	async function finishStreamingChatMessage(
		requestId: string,
		tools: { name: string; details: string }[] = []
	) {
		// Tauri events are global. Ignore a late completion from an older request;
		// otherwise it can close the current bubble while its sidecar stream is
		// still running and allow another request to race with it.
		if (activeChatRequestId !== requestId) return;
		// Flush any chat deltas still buffered for the next frame so the final
		// token(s) are part of the finished bubble before it's marked done.
		flushChatChunks();
		const requestNoteId = activeChatNoteId;
		// A cancelled request can still arrive as chat_done. Revert any speculative
		// editor preview unless note_written already committed the authoritative body.
		cancelNoteStream();
		chatMessages = chatMessages.map((m) => {
			if (m.isStreaming)
				return {
					...m,
					isStreaming: false,
					statusText: undefined,
					endTime: Date.now(),
					debugTrace: pendingDebugTrace
				};
			return m;
		});
		// Keep the request marked active until persistence completes. Rewind/retry
		// waits on this flag; otherwise its newer history can race an older save.
		if (chatPersistTimer) {
			clearTimeout(chatPersistTimer);
			chatPersistTimer = undefined;
		}
		if (requestNoteId) await persistChatHistory(requestNoteId, chatMessages);
		if (requestNoteId && hasNoteMutation(tools)) {
			try {
				await reconcileRequestNote(requestNoteId);
			} catch (error) {
				console.error('Failed to reconcile completed note mutation:', error);
			}
		}
		activeChatRequestId = null;
		activeAiComposerMode = null;
		activeAiEditTarget = null;
		activeChatNoteId = null;
	}

	function extractChatErrorMessage(error: unknown): string {
		if (typeof error === 'string' && error.trim()) return error;
		if (
			error &&
			typeof error === 'object' &&
			'message' in error &&
			typeof error.message === 'string' &&
			error.message.trim()
		) {
			return error.message;
		}
		return 'Failed to generate response.';
	}

	function failStreamingChatMessage(
		requestId: string,
		errorMsg: string,
		tools: { name: string; details: string }[] = []
	) {
		// Tauri events are global; do not let an older request fail the current
		// assistant bubble.
		if (activeChatRequestId !== requestId) return;
		const previewWasReverted = noteStreaming;
		if (previewWasReverted && !errorMsg.includes('Live preview reverted; no changes were saved.')) {
			errorMsg += ' Live preview reverted; no changes were saved.';
		}
		if (showDebugWindow && debugInfo) {
			const finishedAt = Date.now();
			debugInfo = {
				...debugInfo,
				done: finishedAt,
				generationEnd: debugInfo.generationStart ? finishedAt : debugInfo.generationEnd,
				trace: [...debugInfo.trace, { time: finishedAt, msg: `Error: ${errorMsg}`, kind: 'error' }]
			};
		}
		activeChatRequestId = null;
		activeChatNoteId = null;
		activeAiComposerMode = null;
		// If a live note stream was interrupted, the note was never saved —
		// restore the pre-stream content rather than leaving a partial draft.
		cancelNoteStream();
		activeAiEditTarget = null;
		chatMessages = chatMessages.map((m) => {
			if (m.isStreaming) {
				return {
					...m,
					isStreaming: false,
					statusText: undefined,
					error: true,
					content: m.content + '\n\n' + errorMsg,
					tools,
					endTime: Date.now()
				};
			}
			return m;
		});
		if (chatPersistTimer) {
			clearTimeout(chatPersistTimer);
			chatPersistTimer = undefined;
		}
		const aiNoteId = activeAiNoteId();
		if (aiNoteId) void persistChatHistory(aiNoteId, chatMessages);
	}

	async function resolveApproval(id: string, approved: boolean) {
		chatMessages = chatMessages.map((m) => {
			if (m.isApprovalRequest && m.approvalId === id) {
				return { ...m, approvalStatus: approved ? 'approved' : 'rejected' };
			}
			return m;
		});
		await invoke('resolve_tool_approval', { id, approved });
	}

	async function fetchNoteHistory() {
		if (!note) return;
		isBusy = true;
		try {
			const history = await invoke<GitCommit[]>('get_note_history', { noteId: note.id });
			noteHistory = history
				.filter((c) => c.message && c.message.trim() !== '')
				.sort((a, b) => new Date(b.timestamp).getTime() - new Date(a.timestamp).getTime());
		} catch (e) {
			console.error('Failed to fetch history:', e);
		} finally {
			isBusy = false;
		}
	}

	async function previewVersion(commitHash: string) {
		if (!note) return;
		isBusy = true;
		try {
			let rawContent = await invoke<string>('get_note_version', { noteId: note.id, commitHash });
			if (rawContent.match(/^---\r?\n/)) {
				const match = rawContent.match(/^---\r?\n[\s\S]*?\n---\r?\n/);
				if (match) {
					rawContent = rawContent.slice(match[0].length);
				}
			}
			versionPreviewContent = rawContent;
			versionPreviewHash = commitHash;
			if (versionPreviewDialog) {
				versionPreviewDialog.showModal();
			}
		} catch (e) {
			console.error('Failed to fetch version:', e);
		} finally {
			isBusy = false;
		}
	}

	async function restoreVersion(commitHash: string) {
		if (!note) return;
		isBusy = true;
		try {
			let rawContent = await invoke<string>('get_note_version', { noteId: note.id, commitHash });
			if (rawContent.match(/^---\r?\n/)) {
				const match = rawContent.match(/^---\r?\n[\s\S]*?\n---\r?\n/);
				if (match) {
					rawContent = rawContent.slice(match[0].length);
				}
			}
			draftBody = rawContent;
			if (vditorInstance) {
				vditorInstance.setValue(rawContent);
			}
			versionPreviewContent = null;
			versionPreviewHash = null;
			if (versionPreviewDialog) {
				versionPreviewDialog.close();
			}
			triggerAutoSave();
			activeSidebarTab = 'info';
		} catch (e) {
			console.error('Failed to restore version:', e);
		} finally {
			isBusy = false;
		}
	}

	let isProgrammaticNavigation = false;

	function safeNavigate(url: string) {
		if (saveStatus === 'saving' || saveStatus === 'unsaved') {
			pendingNavigationUrl = url;
			navigationWarningDialog?.showModal();
			return;
		}
		isProgrammaticNavigation = true;
		void goto(url);
	}

	// Back button: go to the page the user actually came from (browser history),
	// not always home. A deliberate ?returnTo= still wins, and the unsaved-changes
	// guard is respected (warn first, then go back on confirm).
	function goBack() {
		if (page.url.searchParams.has('returnTo')) {
			safeNavigate(backUrl);
			return;
		}
		if (saveStatus === 'saving' || saveStatus === 'unsaved') {
			pendingBack = true;
			navigationWarningDialog?.showModal();
			return;
		}
		navigateBack();
	}

	function navigateBack() {
		// Mark programmatic so beforeNavigate doesn't re-prompt on the popstate.
		isProgrammaticNavigation = true;
		if (typeof window !== 'undefined' && window.history.length > 1) {
			history.back();
		} else {
			void goto('/');
		}
	}

	function requestDeleteAttachedNote() {
		deleteAttachedNoteDialog?.showModal();
	}

	async function confirmDeleteAttachedNote() {
		deleteAttachedNoteDialog?.close();
		const targetId = isSourceMaterial ? scratchpadSavedId : note?.sourcePdf ? note.id : null;
		const sourceId = isSourceMaterial ? activeSourceId : (note?.sourcePdf ?? activeSourceId);
		isBusy = true;
		try {
			if (targetId) {
				await invoke('delete_note', { noteId: targetId });
			}
			if (!isSourceMaterial && sourceId) {
				isProgrammaticNavigation = true;
				await goto(`/notes/${encodeURIComponent(sourceId)}`);
				return;
			}
			if (saveTimer) {
				clearTimeout(saveTimer);
				saveTimer = null;
			}
			destroyEditorInstance();
			draftBody = '';
			scratchpadSavedId = null;
			showAttachedNote = false;
			saveStatus = 'saved';
			message = '';
		} finally {
			isBusy = false;
		}
	}

	function cancelDeleteAttachedNote() {
		deleteAttachedNoteDialog?.close();
	}

	async function openAttachPdfDialog() {
		pdfSearchQuery = '';
		pdfSelectedIndex = 0;
		isBusy = true;
		try {
			const allDocs = await invoke<NoteDocument[]>('get_all_note_documents');
			const referenced = new Set(
				allDocs.map((d) => d.sourcePdf).filter((id): id is string => !!id)
			);
			const isCopyName = (d: NoteDocument) => {
				const name = d.relativePath.split(/[\\/]/).pop()?.toLowerCase() ?? '';
				return (
					/ \d+\.(pdf|epub)$/.test(name) ||
					/ [0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\.(pdf|epub)$/.test(name)
				);
			};
			pdfNotesList = allDocs.filter(
				(d) =>
					d.relativePath.toLowerCase().endsWith('.pdf') && !(referenced.has(d.id) && isCopyName(d))
			);
		} catch (err) {
			message = `Failed to load PDFs: ${err}`;
		} finally {
			isBusy = false;
		}
		attachPdfDialog?.showModal();
		setTimeout(() => {
			const input = attachPdfDialog?.querySelector('.link-search-input') as HTMLInputElement | null;
			input?.focus();
		}, 50);
	}

	async function attachPdf(pdfNote: NoteDocument, alreadyImported = false) {
		if (!note) return;
		attachPdfDialog?.close();
		isBusy = true;
		let createdPdfId: string | null = alreadyImported ? pdfNote.id : null;
		try {
			const attachmentPdf = alreadyImported
				? pdfNote
				: await invoke<NoteDocument>('clone_pdf_for_attachment', {
						noteId: pdfNote.id,
						notebook: openNoteNotebook()
					});
			createdPdfId = attachmentPdf.id;
			const saved = await invoke<NoteDocument>('save_note', {
				noteId: note.id,
				title: draftTitle,
				tags: draftTags
					.split(',')
					.map((t: string) => t.trim())
					.filter(Boolean),
				body: draftBody,
				sourcePdf: attachmentPdf.id,
				annotations: note.annotations
			});
			note = saved;
			activeSourceId = attachmentPdf.id;
			const bytes = await invoke<ArrayBuffer>('read_pdf_binary', { noteId: attachmentPdf.id });
			activeSourceBytes = new Uint8Array(bytes);
			sourceMaterialType = 'pdf';
			showAttachedNote = true;
			saveStatus = 'saved';
			destroyEditorInstance();
			await tick();
			initVditor();
		} catch (err) {
			if (createdPdfId) {
				try {
					await invoke('delete_note', { noteId: createdPdfId });
				} catch (cleanupError) {
					console.warn('Failed to clean up copied PDF after attachment failure', cleanupError);
				}
			}
			message = `Failed to attach PDF: ${err}`;
		} finally {
			isBusy = false;
		}
	}

	function requestDetachPdf() {
		detachPdfDialog?.showModal();
	}

	async function confirmDetachPdf() {
		detachPdfDialog?.close();
		if (!note) return;
		isBusy = true;
		try {
			const saved = await invoke<NoteDocument>('save_note', {
				noteId: note.id,
				title: draftTitle,
				tags: draftTags
					.split(',')
					.map((t: string) => t.trim())
					.filter(Boolean),
				body: draftBody,
				sourcePdf: null,
				annotations: note.annotations
			});
			note = saved;
			activeSourceId = null;
			activeSourceBytes = null;
			saveStatus = 'saved';
			destroyEditorInstance();
			await tick();
			initVditor();
		} catch (err) {
			message = `Failed to detach PDF: ${err}`;
		} finally {
			isBusy = false;
		}
	}

	async function browseAndAttachPdf() {
		const selected = await openFileDialog({
			multiple: false,
			filters: [{ name: 'Documents', extensions: ['pdf', 'epub', 'tex', 'ipynb', 'md'] }]
		});
		if (!selected) return;
		const filePath = selected;
		attachPdfDialog?.close();
		isBusy = true;
		try {
			const pdfNote = await invoke<NoteDocument>('import_pdf_file', {
				filePath,
				notebook: openNoteNotebook()
			});
			await attachPdf(pdfNote, true);
		} catch (err) {
			message = `Failed to import PDF: ${err}`;
			isBusy = false;
		}
	}

	function handlePdfSearchKeydown(e: KeyboardEvent) {
		if (e.key === 'ArrowDown') {
			e.preventDefault();
			pdfSelectedIndex = Math.min(filteredPdfs.length - 1, pdfSelectedIndex + 1);
		} else if (e.key === 'ArrowUp') {
			e.preventDefault();
			pdfSelectedIndex = Math.max(0, pdfSelectedIndex - 1);
		} else if (e.key === 'Enter') {
			e.preventDefault();
			if (filteredPdfs.length > 0) attachPdf(filteredPdfs[pdfSelectedIndex]);
		}
	}

	function buildPreviewExpandHref() {
		const targetId = previewNoteTarget?.sourcePdf ?? previewNoteTarget?.id;
		const currentNoteId = note?.id;
		if (!targetId) return null;
		const basePath = `/notes/${encodeURIComponent(targetId)}`;
		if (!currentNoteId) return basePath;
		return `${basePath}?returnTo=/notes/${encodeURIComponent(currentNoteId)}`;
	}

	function expandPreviewNoteDirect() {
		const href = buildPreviewExpandHref();
		if (!href) return;
		previewNoteDialog?.close();
		isProgrammaticNavigation = true;
		window.location.href = href;
	}

	function handleBeforeUnload(e: BeforeUnloadEvent) {
		if (isProgrammaticNavigation) return;
		if (saveStatus === 'saving' || saveStatus === 'unsaved') {
			e.preventDefault();
			e.returnValue = '';
		}
	}

	beforeNavigate(({ cancel, to }) => {
		if (isProgrammaticNavigation) return;
		if (saveStatus === 'saving' || saveStatus === 'unsaved') {
			pendingNavigationUrl = to?.url ? `${to.url.pathname}${to.url.search}${to.url.hash}` : '';
			navigationWarningDialog?.showModal();
			cancel();
		}
	});

	function confirmNavigation() {
		navigationWarningDialog?.close();
		if (pendingBack) {
			pendingBack = false;
			navigateBack();
			return;
		}
		if (pendingNavigationUrl) {
			isProgrammaticNavigation = true;
			void goto(pendingNavigationUrl);
			pendingNavigationUrl = '';
		}
	}

	function cancelNavigation() {
		navigationWarningDialog?.close();
		pendingNavigationUrl = '';
		pendingBack = false;
	}

	function updateToolbarOverflow() {
		const toolbar = vditorContainer?.querySelector('.vditor-toolbar');
		if (!toolbar) return;
		const items = toolbar.querySelectorAll('.vditor-toolbar__item, .vditor-toolbar__divider');
		items.forEach((item: any) => {
			if (!toolbarExpanded && item.offsetTop > 20) {
				item.style.visibility = 'hidden';
				item.style.pointerEvents = 'none';
			} else {
				item.style.visibility = 'visible';
				item.style.pointerEvents = 'auto';
			}
		});
	}

	$effect(() => {
		const _trigger = toolbarExpanded;
		updateToolbarOverflow();
	});

	function handleGlobalSelectionChange() {
		// Streaming rebuilds the editor DOM and restores the caret programmatically,
		// which fires selectionchange; running the full querySelectorAll + onSelection
		// capture (including a full vditorInstance.getValue()) every ~120ms during
		// a note stream is wasted work. Skip until the stream settles.
		if (noteStreaming) return;
		if (!vditorContainer) return;
		const sel = window.getSelection();

		// Clean up previous expansion
		vditorContainer.querySelectorAll('.force-expand').forEach((el) => {
			el.classList.remove('force-expand');
		});

		if (!sel || sel.rangeCount === 0) return;

		// Arm cursor and selection targets for Write mode (debounced so the
		// browser has committed the final drag/caret position).
		onSelectionChange();

		// Link expansion is only relevant for a non-empty selection.
		if (sel.isCollapsed) return;

		// Expand links that intersect the current selection
		const links = vditorContainer.querySelectorAll('[data-type="a"]');
		links.forEach((link) => {
			if (sel.containsNode(link, true)) {
				link.classList.add('force-expand');
			}
		});
	}

	onMount(() => {
		// Warm llama-server (safety net — the server is already started at app
		// boot and stays warm for the entire session).
		const savedInteractionMode = localStorage.getItem('myelin_ai_interaction_mode');
		aiInteractionMode = savedInteractionMode === 'operation' ? 'operation' : 'chat';
		const savedSidebarWidth = localStorage.getItem('myelin_sidebar_width');
		if (savedSidebarWidth) {
			const parsed = parseInt(savedSidebarWidth, 10);
			if (!isNaN(parsed)) sidebarWidth = parsed;
		}

		let unlistenChunk: UnlistenFn;
		let unlistenDone: UnlistenFn;
		let unlistenError: UnlistenFn;
		let unlistenUsage: UnlistenFn;
		let unlistenApproval: UnlistenFn;
		let unlistenNoteWritten: UnlistenFn;
		let unlistenNoteStreamStart: UnlistenFn;
		let unlistenNoteDelta: UnlistenFn;
		let unlistenNoteStreamCancel: UnlistenFn;
		let unlistenLatex: UnlistenFn;
		let unlistenAiWarmup: UnlistenFn;

		$showSidebarToggle = true;
		// The note sidebar's open/closed state is remembered across sessions via the
		// persisted noteSidebarOpen store, so we intentionally don't force it here.

		const mql = window.matchMedia('(max-width: 1200px)');
		const handleMediaChange = (_e: MediaQueryListEvent) => {};
		mql.addEventListener('change', handleMediaChange);
		document.addEventListener('selectionchange', handleGlobalSelectionChange);
		document.addEventListener('mousedown', onDocMouseDown, true);
		window.addEventListener('keydown', handleChatSidebarShortcut, true);
		let unlistenTool: () => void;

		// Setup AI Streaming listeners
		listen<{ noteId: string; content: string; mode: 'write' | 'append' }>(
			'ai://note_written',
			(event) => {
				const { noteId, content, mode } = event.payload;
				if (!note || activeAiNoteId() !== noteId) return;
				applyNoteWrite(content, mode);
				if (activeChatRequestId && showDebugWindow && debugInfo) {
					debugInfo = {
						...debugInfo,
						trace: [
							...debugInfo.trace,
							{
								time: Date.now(),
								msg: `Note written (${content.length}c ${mode})`,
								kind: 'note' as const
							}
						]
					};
				}
				// Cursor anchors are consumed by an insertion. Selected spans are
				// refreshed so an immediate follow-up can target the replacement.
				if (armedSelection?.cursor || workingDocType !== 'md') clearArmedSelection();
				else if (armedSelection) setTimeout(reselectAfterEdit, 60);
			}
		).then((fn) => (unlistenNoteWritten = fn));

		listen<{ noteId: string; requestId: string }>('ai://note_stream_start', (event) => {
			if (
				!note ||
				activeAiNoteId() !== event.payload.noteId ||
				activeChatRequestId !== event.payload.requestId
			)
				return;
			beginNoteStream();
			if (showDebugWindow && debugInfo) {
				debugInfo = {
					...debugInfo,
					trace: [
						...debugInfo.trace,
						{ time: Date.now(), msg: 'Streaming note to editor…', kind: 'note' as const }
					]
				};
			}
		}).then((fn) => (unlistenNoteStreamStart = fn));

		listen<{ noteId: string; requestId: string; delta: string }>('ai://note_delta', (event) => {
			if (
				!note ||
				activeAiNoteId() !== event.payload.noteId ||
				activeChatRequestId !== event.payload.requestId
			)
				return;
			appendNoteStream(event.payload.delta);
			setStreamingStatus('Writing replacement…');
		}).then((fn) => (unlistenNoteDelta = fn));

		listen<{ noteId: string; requestId: string }>('ai://note_stream_cancel', (event) => {
			if (
				!note ||
				activeAiNoteId() !== event.payload.noteId ||
				activeChatRequestId !== event.payload.requestId
			)
				return;
			cancelNoteStream();
		}).then((fn) => (unlistenNoteStreamCancel = fn));

		listen<{ tool: string; details: string; mutatesNote?: boolean }>('ai://chat_tool', (event) => {
			if (!activeChatRequestId) return;
			let lastStartTime = Date.now();
			const toolStatus =
				activeAiComposerMode === 'editor' && event.payload.mutatesNote
					? 'Applying selected edit…'
					: `Using ${event.payload.tool.toLowerCase()}…`;
			if (showDebugWindow && debugInfo) {
				debugInfo = {
					...debugInfo,
					trace: [
						...debugInfo.trace,
						{ time: Date.now(), msg: `Tool: ${event.payload.tool}`, kind: 'tool' as const }
					]
				};
			}
			chatMessages = chatMessages.map((m) => {
				if (m.isStreaming) {
					lastStartTime = m.startTime || lastStartTime;
					// On a note edit, drop the model's pre-tool prose — it tends to
					// duplicate the note content that's already shown in the editor.
					return { ...m, isStreaming: false, content: event.payload.mutatesNote ? '' : m.content };
				}
				return m;
			});
			chatMessages = [
				...chatMessages,
				{
					role: 'assistant',
					content: '',
					tools: [{ name: event.payload.tool, details: event.payload.details }],
					isStreaming: false
				},
				{
					role: 'assistant',
					content: '',
					isStreaming: true,
					startTime: lastStartTime,
					statusText: toolStatus
				}
			];
			if (chatMessagesEl) {
				setTimeout(() => scrollChatToBottom(true), 100);
			}
		}).then((fn) => (unlistenTool = fn));

		listen<{ id: string; tool: string; title: string; content: string }>(
			'ai://tool_approval_request',
			(event) => {
				let lastStartTime = Date.now();
				chatMessages = chatMessages.map((m) => {
					if (m.isStreaming) {
						lastStartTime = m.startTime || lastStartTime;
						return { ...m, isStreaming: false };
					}
					return m;
				});
				chatMessages = [
					...chatMessages,
					{
						role: 'assistant',
						content: '',
						isApprovalRequest: true,
						approvalId: event.payload.id,
						approvalTool: event.payload.tool,
						approvalDetails: `Title: ${event.payload.title}\nContent:\n${event.payload.content}`,
						approvalStatus: 'pending'
					},
					{
						role: 'assistant',
						content: '',
						isStreaming: true,
						startTime: lastStartTime
					}
				];
				if (chatMessagesEl) {
					setTimeout(() => {
						scrollChatToBottom(true);
					}, 100);
				}
			}
		).then((fn) => (unlistenApproval = fn));

		listen<{ delta: string; requestId: string }>('ai://chat_chunk', (event) => {
			if (activeChatRequestId !== event.payload.requestId) return;
			// Buffer deltas and apply once per frame; applying on every token
			// re-renders the whole streaming bubble (full markdown re-parse).
			chatChunkBuf += event.payload.delta;
			if (!chatChunkFlushPending) {
				chatChunkFlushPending = true;
				requestAnimationFrame(() => {
					chatChunkFlushPending = false;
					flushChatChunks();
				});
			}
		}).then((fn) => (unlistenChunk = fn));

		listen<{ requestId: string; tools?: { name: string; details: string }[] }>(
			'ai://chat_done',
			(event) => {
				void finishStreamingChatMessage(event.payload.requestId, event.payload.tools || []);
				if (activeChatRequestId === event.payload.requestId && showDebugWindow && debugInfo) {
					debugInfo = {
						...debugInfo,
						done: Date.now(),
						trace: [...debugInfo.trace, { time: Date.now(), msg: 'Done', kind: 'done' as const }]
					};
				}
			}
		).then((fn) => (unlistenDone = fn));

		listen<{
			requestId: string;
			promptTokens: number;
			completionTokens: number;
			totalTokens: number;
		}>('ai://chat_usage', (event) => {
			if (activeChatRequestId !== event.payload.requestId) return;
			if (showDebugWindow && debugInfo) {
				debugInfo = {
					...debugInfo,
					promptTokens: event.payload.promptTokens,
					completionTokens: event.payload.completionTokens,
					totalTokens: event.payload.totalTokens
				};
				debugInfo.generationEnd = Date.now();
			}
		}).then((fn) => (unlistenUsage = fn));

		listen<{ requestId: string; message: string; tools?: { name: string; details: string }[] }>(
			'ai://chat_error',
			(event) => {
				failStreamingChatMessage(
					event.payload.requestId,
					event.payload.message,
					event.payload.tools || []
				);
			}
		).then((fn) => (unlistenError = fn));

		listen<{ status: 'started' | 'ready' | 'failed'; message?: string }>(
			'ai://llama_warmup',
			(event) => {
				if (!activeChatRequestId) return;
				if (event.payload.status === 'started') {
					setStreamingStatus('Reading the note… warming the model…');
				} else if (event.payload.status === 'ready') {
					setStreamingStatus('Model ready — preparing the response…');
				}
			}
		).then((fn) => (unlistenAiWarmup = fn));

		// Debug event: model behavior, tool calls, grammar config, etc.
		let unlistenDebug: UnlistenFn | undefined;
		listen<{ kind: string; msg: string; requestId: string }>('ai://debug_event', (event) => {
			if (activeChatRequestId !== event.payload.requestId) return;
			const entry = makeDebugTraceEntry(event.payload.kind, event.payload.msg);
			// Keep every trace even with the panel closed; it is attached to
			// the completed assistant turn and persisted in chat history.
			pendingDebugTrace = [...pendingDebugTrace.slice(-(MAX_DEBUG_TRACE - 1)), entry];
			const status = visibleAiStatus(event.payload.kind, event.payload.msg);
			if (status) setStreamingStatus(status);
			if (showDebugWindow && debugInfo) {
				const isModelStart =
					event.payload.kind === 'gen' || event.payload.kind === 'first_model_delta';
				debugInfo = {
					...debugInfo,
					generationStart: isModelStart ? entry.time : debugInfo.generationStart,
					firstChunk:
						event.payload.kind === 'gen' && debugInfo.firstChunk === null
							? entry.time
							: debugInfo.firstChunk,
					trace: [...debugInfo.trace.slice(-(MAX_DEBUG_TRACE - 1)), entry]
				};
			}
		}).then((fn) => (unlistenDebug = fn));

		// LaTeX support bundle download progress (first compile only).
		listen<{ phase: string; bytes?: number; message?: string }>('latex://download', (event) => {
			const p = event.payload;
			const mb = ((p.bytes ?? 0) / (1024 * 1024)).toFixed(1);
			if (p.phase === 'start' || p.phase === 'progress') {
				latexDownloadMsg = `Downloading LaTeX support files (first run)… ${mb} MB`;
			} else if (p.phase === 'done') {
				texCacheWarmed = true;
				latexDownloadMsg = null;
			} else if (p.phase === 'error') {
				latexDownloadMsg = null;
			}
		}).then((fn) => (unlistenLatex = fn));
		invoke<{ warmed: boolean }>('tectonic_cache_status')
			.then((status) => (texCacheWarmed = status.warmed))
			.catch(() => {});

		window.addEventListener('mousemove', handleGlobalMouseMove);
		window.addEventListener('mouseup', stopResizing);
		window.addEventListener('beforeunload', handleBeforeUnload);

		return () => {
			mql.removeEventListener('change', handleMediaChange);
			document.removeEventListener('selectionchange', handleGlobalSelectionChange);
			document.removeEventListener('mousedown', onDocMouseDown, true);
			window.removeEventListener('keydown', handleChatSidebarShortcut, true);
			window.removeEventListener('mousemove', handleGlobalMouseMove);
			window.removeEventListener('mouseup', stopResizing);
			window.removeEventListener('beforeunload', handleBeforeUnload);
			$showSidebarToggle = false;

			if (unlistenChunk) unlistenChunk();
			if (unlistenDone) unlistenDone();
			if (unlistenError) unlistenError();
			if (unlistenUsage) unlistenUsage();
			if (unlistenTool) unlistenTool();
			if (unlistenApproval) unlistenApproval();
			if (unlistenDebug) unlistenDebug();
			if (unlistenNoteWritten) unlistenNoteWritten();
			if (unlistenNoteStreamStart) unlistenNoteStreamStart();
			if (unlistenNoteDelta) unlistenNoteDelta();
			if (unlistenNoteStreamCancel) unlistenNoteStreamCancel();
			if (unlistenLatex) unlistenLatex();
			if (unlistenAiWarmup) unlistenAiWarmup();
		};
	});

	onDestroy(() => {
		// Note view closing — server stays warm (started at app boot, lives until
		// app exit). Only the note-editor UI is torn down.
		if (chatPersistTimer) clearTimeout(chatPersistTimer);
		const aiNoteId = activeAiNoteId();
		if (aiNoteId && chatMessages.length) void persistChatHistory(aiNoteId, chatMessages);
		noteClosed();
		if (toolbarResizeObserver) toolbarResizeObserver.disconnect();
		if (vditorInstance) vditorInstance.destroy();
		if (typeof document !== 'undefined') {
			document.removeEventListener('selectionchange', handleGlobalSelectionChange);
			document.removeEventListener('mousedown', onDocMouseDown, true);
		}
	});

	$effect(() => {
		const routeNoteId = page.params.id;
		if (!routeNoteId || routeNoteId === loadedRouteNoteId) return;
		loadedRouteNoteId = routeNoteId;
		void loadCurrentNote(routeNoteId);
	});

	// Debug window: update the live elapsed timer while a request is in progress.
	let debugTraceEl: HTMLDivElement | undefined = $state();
	$effect(() => {
		if (debugInfo && debugTraceEl) {
			debugTraceEl.scrollTop = debugTraceEl.scrollHeight;
		}
	});
	$effect(() => {
		// Only keep the 100ms live-elapsed ticker while the debug window is open
		// AND a request is in flight; the window already renders wall-clock time.
		if (!showDebugWindow || !debugInfo || debugInfo.done || !activeChatRequestId) {
			if (debugTimer) {
				clearInterval(debugTimer);
				debugTimer = null;
			}
			return;
		}
		if (!debugTimer) {
			debugTimer = setInterval(() => {
				if (debugInfo && !debugInfo.done) {
					debugInfo = { ...debugInfo };
				}
			}, 100);
		}
	});
</script>

<svelte:head>
	<title>{note ? note.title : 'Myelin'}</title>
	<style>
		/* Bruteforce hide the left scrollbar in split view to prevent Svelte scoping issues */
		.vditor-sv::-webkit-scrollbar {
			display: none !important;
			width: 0 !important;
			background: transparent !important;
		}
		.vditor-sv {
			scrollbar-width: none !important;
			-ms-overflow-style: none !important;
		}
	</style>
</svelte:head>

<div
	class="editor-shell"
	class:has-attached-file={!!note?.sourcePdf || (isSourceMaterial && !!activeSourceBytes)}
	class:resizing={isResizing || isSidebarResizing}
>
	<header class="editor-header">
		<div class="header-copy">
			<button class="back-link" onclick={goBack} aria-label="Go back" title="Go back">
				<svg
					viewBox="0 0 24 24"
					width="20"
					height="20"
					stroke="currentColor"
					stroke-width="2"
					fill="none"
					stroke-linecap="round"
					stroke-linejoin="round"
				>
					<line x1="19" y1="12" x2="5" y2="12"></line>
					<polyline points="12 19 5 12 12 5"></polyline>
				</svg>
			</button>
			{#if message}
				<p class="status">{message}</p>
			{/if}
			<input
				class="title-input"
				bind:value={draftTitle}
				oninput={triggerAutoSave}
				placeholder="Note title"
			/>

			<div class="save-indicator" class:saving={saveStatus === 'saving'}>
				{#if saveStatus === 'saving'}
					<svg
						class="spinner"
						viewBox="0 0 24 24"
						width="14"
						height="14"
						stroke="currentColor"
						stroke-width="2"
						fill="none"
						stroke-linecap="round"
						stroke-linejoin="round"
						><circle cx="12" cy="12" r="10"></circle><path d="M12 6v6l4 2"></path></svg
					> Saving
				{:else if saveStatus === 'unsaved'}
					<span class="dot"></span> Unsaved
				{:else}
					<svg
						viewBox="0 0 24 24"
						width="14"
						height="14"
						stroke="currentColor"
						stroke-width="2"
						fill="none"
						stroke-linecap="round"
						stroke-linejoin="round"><polyline points="20 6 9 17 4 12"></polyline></svg
					> Saved
				{/if}
			</div>
		</div>
	</header>

	{#if isLoadingNote}
		<div class="note-loading" role="status" aria-live="polite">
			<svg
				class="note-loading-spinner"
				viewBox="0 0 24 24"
				width="18"
				height="18"
				fill="none"
				stroke="currentColor"
				stroke-width="2"
				stroke-linecap="round"
			>
				<path d="M21 12a9 9 0 1 1-6.219-8.56" />
			</svg>
			<span>Opening note…</span>
		</div>
	{/if}

	<div
		class="main-layout"
		class:split-layout={activeSourceBytes !== null && showAttachedNote}
		bind:this={mainLayoutEl}
	>
		{#if activeSourceBytes}
			<section
				class="pdf-pane"
				class:tex-pane={workingDocType === 'tex'}
				style="position: relative; width: {!showAttachedNote ? '100%' : `${splitRatio}%`}"
			>
				{#if workingDocType === 'tex'}
					<div class="tex-pane-badge">PDF preview</div>
				{/if}
				{#if sourceMaterialType === 'pdf' && PdfViewerComponent}
					<PdfViewerComponent
						pdfBytes={activeSourceBytes}
						annotations={note?.annotations || []}
						onQuote={handlePdfQuote}
						onAnnotationsChange={handleAnnotationsChange}
						onImageExtract={handleImageExtract}
						onTextExtracted={handlePdfTextExtracted}
						onClosePdf={workingDocType === 'tex'
							? closeTexPreview
							: activeSourceBytes !== null && showAttachedNote
								? requestDetachPdf
								: undefined}
						onAttachNote={() => void openAttachedNote()}
						showAttachButton={!showAttachedNote}
					/>
				{:else if sourceMaterialType === 'pdf'}
					<div class="viewer-loading">Loading PDF viewer…</div>
				{:else if sourceMaterialType === 'epub' && EpubViewerComponent}
					<EpubViewerComponent epubBytes={activeSourceBytes} />
					{#if !showAttachedNote}
						<button
							style="position: absolute; top: 10px; right: 10px;"
							class="primary"
							onclick={() => {
								showAttachedNote = true;
								setTimeout(() => initVditor(), 100);
							}}>Attach Note</button
						>
					{/if}
				{:else if sourceMaterialType === 'epub'}
					<div class="viewer-loading">Loading EPUB viewer…</div>
				{:else if sourceMaterialType === 'html' && HtmlViewerComponent}
					<HtmlViewerComponent htmlBytes={activeSourceBytes} />
					{#if !showAttachedNote}
						<button
							style="position: absolute; top: 10px; right: 10px;"
							class="primary"
							onclick={() => {
								showAttachedNote = true;
								setTimeout(() => initVditor(), 100);
							}}>Attach Note</button
						>
					{/if}
				{:else if sourceMaterialType === 'html'}
					<div class="viewer-loading">Loading document viewer…</div>
				{/if}
				{#if sourceMaterialType === 'pdf' && (pdfIngestionStatus === 'indexing' || pdfIngestionStatus === 'empty' || pdfIngestionStatus === 'failed')}
					<div
						class="pdf-index-status"
						class:error={pdfIngestionStatus === 'failed'}
						title={pdfIngestionError ?? undefined}
					>
						{pdfIngestionStatus === 'indexing'
							? 'Indexing PDF for AI…'
							: pdfIngestionStatus === 'empty'
								? 'No selectable PDF text to index'
								: 'PDF indexing failed'}
					</div>
				{/if}
			</section>
			{#if showAttachedNote}
				<!-- svelte-ignore a11y_no_static_element_interactions -->
				<div class="resizer" onmousedown={startResizing} class:resizing={isResizing}></div>
			{/if}
		{/if}

		<!-- Main Content Area -->
		{#if shouldRenderEditor}
			<section
				class="main-pane"
				class:tex-pane={workingDocType === 'tex'}
				style={activeSourceBytes ? `width: ${100 - splitRatio}%` : ''}
			>
				<div class="content-area" style="position: relative;">
					{#if workingDocType === 'md'}
						<!-- svelte-ignore a11y_click_events_have_key_events -->
						<!-- svelte-ignore a11y_no_static_element_interactions -->
						<div
							bind:this={vditorContainer}
							class="vditor-wrapper"
							class:tools-loading={!toolsReady || vditorLoading}
							class:toolbar-expanded={toolbarExpanded}
							class:has-pdf-note={!!activeSourceBytes || (!isSourceMaterial && !!note)}
							onclickcapture={handleVditorClick}
							onkeydowncapture={handleVditorKeydownCapture}
							onkeyupcapture={handleVditorKeyupCapture}
							onwheelcapture={(e) => {
								if (e.ctrlKey || e.metaKey) {
									e.preventDefault();
									e.stopPropagation();
								}
							}}
						>
							{#if !toolsReady || vditorLoading}
								<div class="tools-loading-overlay" aria-live="polite">Loading editing tools…</div>
							{/if}
						</div>
						<div class="fullscreen-indicator">
							Press <span>{fullscreenShortcut}</span> to toggle
						</div>
					{:else if workingDocType === 'tex' && TexEditorComponent}
						<TexEditorComponent
							bind:this={texEditorInstance}
							value={draftBody}
							onInput={(val: string) => {
								lastTexBody = val;
								texRevision += 1;
								if (texAutoCompile && texCacheWarmed) texPreviewStatus = 'pending';
								draftBody = val;
								triggerAutoSave();
							}}
							diagnostics={texDiagnostics}
							onPickImage={pickLatexImage}
							onAiTargetChange={captureExternalTarget}
							busy={isBusy}
						/>
					{:else if workingDocType === 'tex'}
						<div class="editor-loading">Loading LaTeX editor…</div>
					{:else if workingDocType === 'ipynb' && IpynbEditorComponent}
						<IpynbEditorComponent
							bind:this={ipynbEditorInstance}
							value={draftBody}
							onInput={(val: string) => {
								draftBody = val;
								triggerAutoSave();
							}}
							onAiTargetChange={captureExternalTarget}
						/>
					{:else}
						<div class="editor-loading">Loading notebook editor…</div>
					{/if}

					{#if isSourceMaterial && activeSourceBytes && showAttachedNote}
						<div
							class="toolbar-close-note-container"
							style={toolbarNeedsToggle ? 'right: 50px;' : 'right: 12px;'}
						>
							<button
								class="toolbar-close-note-btn"
								onclick={requestDeleteAttachedNote}
								disabled={isBusy}
								title="Delete attached note and close pane"
							>
								Close Note
							</button>
						</div>
					{/if}
					<div class="toolbar-note-actions-container">
						{#if toolbarNeedsToggle}
							<button
								class="toolbar-overlay-toggle"
								class:expanded={toolbarExpanded}
								onclick={() => (toolbarExpanded = !toolbarExpanded)}
								aria-label="Toggle toolbar"
							>
								<svg
									viewBox="0 0 24 24"
									width="16"
									height="16"
									stroke="currentColor"
									stroke-width="2"
									fill="none"
									stroke-linecap="round"
									stroke-linejoin="round"
								>
									<polyline points="6 9 12 15 18 9"></polyline>
								</svg>
							</button>
						{/if}
						{#if activeSourceBytes !== null && showAttachedNote}
							<button
								class="toolbar-overlay-toggle"
								onclick={requestDeleteMainNote}
								aria-label="Delete Note"
								title="Delete Note"
							>
								<svg
									viewBox="0 0 24 24"
									width="14"
									height="14"
									stroke="var(--danger, #ef4444)"
									stroke-width="2"
									fill="none"
									stroke-linecap="round"
									stroke-linejoin="round"
								>
									<line x1="18" y1="6" x2="6" y2="18"></line>
									<line x1="6" y1="6" x2="18" y2="18"></line>
								</svg>
							</button>
						{/if}
					</div>
				</div>
			</section>

			<!-- Right Sidebar -->
			{#if $noteSidebarOpen}
				<!-- svelte-ignore a11y_click_events_have_key_events -->
				<!-- svelte-ignore a11y_no_static_element_interactions -->
				<div class="sidebar-backdrop" onclick={() => ($noteSidebarOpen = false)}></div>
			{/if}
			<aside
				class="sidebar"
				class:open={$noteSidebarOpen}
				style="--sidebar-width: {sidebarWidth}px;"
			>
				<!-- svelte-ignore a11y_no_static_element_interactions -->
				<div
					class="sidebar-resizer"
					onmousedown={startSidebarResizing}
					class:resizing={isSidebarResizing}
				></div>
				<div class="sidebar-tabs">
					<button
						class:active={activeSidebarTab === 'info'}
						onclick={() => (activeSidebarTab = 'info')}>Info</button
					>
					<button
						class:active={activeSidebarTab === 'chat'}
						onclick={() => (activeSidebarTab = 'chat')}>Chat</button
					>
					<button
						class:active={activeSidebarTab === 'versions'}
						onclick={() => {
							activeSidebarTab = 'versions';
							fetchNoteHistory();
						}}>History</button
					>
				</div>

				<div class="sidebar-content">
					{#if activeSidebarTab === 'info'}
						<div class="sidebar-section">
							<h3>Tags</h3>
							<input
								class="tag-input"
								bind:value={draftTags}
								oninput={triggerAutoSave}
								placeholder="comma,separated,tags"
								onblur={fetchRelatedNotes}
							/>
						</div>

						<div class="sidebar-section">
							<h3>Related Notes</h3>
							{#if relatedNotes.length > 0}
								<ul class="related-list">
									{#each relatedNotes as rel, i (rel.id + '_' + i)}
										<li><a href="/notes/{encodeURIComponent(rel.id)}">{rel.title}</a></li>
									{/each}
								</ul>
							{:else}
								<p class="empty-state">No related notes found.</p>
							{/if}
						</div>

						<div class="sidebar-section">
							<h3>Backlinks</h3>
							{#if note && note.backlinks && note.backlinks.length > 0}
								<ul class="related-list">
									{#each note.backlinks as link, i (link.sourceId + '_' + (link.targetBlock || '') + '_' + i)}
										<li>
											<a href="/notes/{encodeURIComponent(link.sourceId)}">
												<strong>{link.sourceTitle}</strong>
												{#if link.targetBlock}
													<span style="opacity: 0.7; font-size: 0.8em;">#{link.targetBlock}</span>
												{/if}
											</a>
											<p
												class="context-excerpt"
												style="font-size: 0.75rem; color: var(--text-secondary); margin-top: 0.25rem; line-height: 1.4;"
											>
												{@html parseBacklinkContext(link.contextExcerpt)}
											</p>
										</li>
									{/each}
								</ul>
							{:else}
								<p class="empty-state">No backlinks yet.</p>
							{/if}
						</div>

						{#if workingDocType === 'tex'}
							<div class="sidebar-section latex-preview-section" aria-live="polite">
								<h3>LaTeX preview</h3>
								<div class="latex-preview-controls">
									<button
										class="secondary latex-auto-toggle"
										onclick={() => (texAutoCompile = !texAutoCompile)}
									>
										Auto: {texAutoCompile ? 'on' : 'off'}
									</button>
									<button
										class="primary"
										disabled={texCompiling}
										onclick={() => void compileTex({ manual: true })}
									>
										{texCompiling ? 'Compiling…' : 'Compile to PDF'}
									</button>
								</div>
								<div class="latex-preview-state" class:error={texPreviewStatus === 'error'}>
									<span
										class="latex-status-dot"
										class:active={texPreviewStatus === 'current'}
										class:pending={texPreviewStatus === 'pending' ||
											texPreviewStatus === 'compiling'}
									></span>
									{#if latexDownloadMsg}
										{latexDownloadMsg}
									{:else if texCompiling}
										Compiling preview…
									{:else if texPreviewStatus === 'pending'}
										Preview pending…
									{:else if texPreviewStatus === 'current'}
										Preview current
									{:else if texPreviewStatus === 'error'}
										Preview error
									{:else}
										Preview idle
									{/if}
								</div>
								{#if texCompileError}
									<p class="latex-error">{texCompileError}</p>
								{/if}
								{#if texDiagnostics.length > 0}
									<ul class="latex-diagnostics">
										{#each texDiagnostics as diagnostic}
											<li>
												<span>{diagnostic.line ? `Line ${diagnostic.line}: ` : ''}</span
												>{diagnostic.message}
											</li>
										{/each}
									</ul>
								{/if}
							</div>
						{/if}
					{:else if activeSidebarTab === 'chat'}
						<div class="chat-container">
							<div class="chat-messages" bind:this={chatMessagesEl} onscroll={handleChatScroll}>
								{#if chatMessages.length === 0}
									<p class="empty-state">Ask me anything about this note or your library!</p>
								{:else}
									{#each chatMessages as msg, i}
										{@const visibleContent =
											msg.role === 'assistant' ? hideThinkingContent(msg.content) : msg.content}
										{#if msg.role === 'user' || visibleContent || (msg.tools && msg.tools.length > 0) || (msg.isApprovalRequest && msg.approvalStatus !== 'approved') || msg.isStreaming || msg.error}
											<div
												class="chat-message {msg.role}"
												class:tool-only={!msg.content &&
													((msg.tools && msg.tools.length > 0) || msg.isApprovalRequest)}
											>
												<div class="chat-bubble" class:error={msg.error}>
													{#if msg.tools && msg.tools.length > 0}
														<div class="chat-tools">
															{#each msg.tools as tool}
																<ChatToolIndicator {tool} />
															{/each}
														</div>
													{/if}
													{#if msg.error}
														<span class="chat-error-text">
															{visibleContent || 'Failed to generate response.'}
														</span>
													{:else if msg.isApprovalRequest && msg.approvalStatus !== 'approved'}
														<ChatToolIndicator
															tool={{
																name:
																	(msg.approvalStatus === 'rejected'
																		? 'Rejected tool: '
																		: 'Pending tool: ') + msg.approvalTool,
																details: msg.approvalDetails || ''
															}}
														/>
													{:else if msg.role === 'assistant' && visibleContent}
														<div class="selectable-content">
															{@html renderChatContent(visibleContent)}
														</div>
													{:else if visibleContent}
														<span class="selectable-content">{visibleContent}</span>
													{/if}
													{#if msg.isStreaming && msg.startTime}
														{#if !msg.content}
															{#if msg.statusText}
																<span class="chat-progress" role="status" aria-live="polite"
																	>{msg.statusText}</span
																>
															{/if}
															<span class="chat-working" aria-label="Working"
																><span></span><span></span><span></span></span
															>
														{/if}
														<span class="chat-time-taken live"
															>{((currentTime - msg.startTime) / 1000).toFixed(1)}s</span
														>
													{:else if msg.endTime && msg.startTime}
														<span class="chat-time-taken"
															>{((msg.endTime - msg.startTime) / 1000).toFixed(1)}s</span
														>
													{/if}
												</div>
												{#if msg.role === 'user' && msg.snapshot}
													<div class="chat-msg-actions">
														<button
															class="rewind-btn"
															onclick={() => rewindToSnapshot(msg.snapshot, msg.content)}
															title="Undo — restore note and put prompt back in input">↩</button
														>
														<button
															class="rewind-btn retry"
															onclick={() => retryMessage(msg.snapshot!, msg.content)}
															title="Retry — rewind and resend this prompt">↻</button
														>
													</div>
												{/if}
												{#if msg.role === 'assistant' && visibleContent && !msg.isStreaming}
													<div class="chat-msg-actions assistant">
														<button
															class="rewind-btn copy-btn"
															onclick={() => copyMessage(i, visibleContent)}
															title="Copy response"
															aria-label="Copy response"
														>
															{#if copiedIdx === i}✓{:else}
																<svg
																	width="13"
																	height="13"
																	viewBox="0 0 24 24"
																	fill="none"
																	stroke="currentColor"
																	stroke-width="2"
																	stroke-linecap="round"
																	stroke-linejoin="round"
																	><rect x="9" y="9" width="13" height="13" rx="2" ry="2" /><path
																		d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"
																	/></svg
																>
															{/if}
														</button>
													</div>
												{/if}
											</div>
										{/if}
									{/each}
								{/if}
							</div>

							{#if chatMessages.find((m) => m.isApprovalRequest && m.approvalStatus === 'pending')}
								{@const pendingReq = chatMessages.find(
									(m) => m.isApprovalRequest && m.approvalStatus === 'pending'
								)}
								<div class="pending-approval-bar">
									<div class="pending-info">
										<span class="tool-icon">⚡</span>
										<span class="pending-text"
											>AI wants to use <strong>{pendingReq?.approvalTool}</strong></span
										>
									</div>
									<div class="pending-actions">
										<button
											class="primary"
											onclick={() => resolveApproval(pendingReq!.approvalId!, true)}>Approve</button
										>
										<button
											class="secondary"
											onclick={() => resolveApproval(pendingReq!.approvalId!, false)}>Reject</button
										>
									</div>
								</div>
							{/if}

							<div class="chat-input-area">
								{#if showDebugWindow && debugInfo}
									<div class="debug-window">
										<div class="debug-window-header">
											<span class="debug-title">AI Debug</span>
											<button
												class="debug-toggle"
												onclick={() => (showDebugWindow = false)}
												title="Close debug window">×</button
											>
										</div>
										<div class="debug-grid">
											<div class="debug-row">
												<span class="debug-label">Prompt → First token:</span>
												<span class="debug-value"
													>{debugInfo.firstChunk && debugInfo.requestStart
														? ((debugInfo.firstChunk - debugInfo.requestStart) / 1000).toFixed(2) +
															's'
														: '—'}</span
												>
											</div>
											<div class="debug-row">
												<span class="debug-label">First token → Done:</span>
												<span class="debug-value"
													>{debugInfo.done && debugInfo.firstChunk
														? ((debugInfo.done - debugInfo.firstChunk) / 1000).toFixed(2) + 's'
														: '—'}</span
												>
											</div>
											<div class="debug-row">
												<span class="debug-label">Total elapsed:</span>
												<span class="debug-value"
													>{debugInfo.done
														? ((debugInfo.done - (debugInfo.requestStart ?? 0)) / 1000).toFixed(2) +
															's'
														: debugInfo.requestStart
															? ((Date.now() - debugInfo.requestStart) / 1000).toFixed(2) +
																's (live)'
															: '—'}</span
												>
											</div>
											<div class="debug-row">
												<span class="debug-label">Prompt tokens:</span>
												<span class="debug-value">{debugInfo.promptTokens || '—'}</span>
											</div>
											<div class="debug-row">
												<span class="debug-label">Completion tokens:</span>
												<span class="debug-value">{debugInfo.completionTokens || '—'}</span>
											</div>
											<div class="debug-row">
												<span class="debug-label">Tokens/s (model):</span>
												<span class="debug-value">
													{#if debugInfo.generationStart && debugInfo.generationEnd}
														{@const genSec =
															(debugInfo.generationEnd - debugInfo.generationStart) / 1000}
														{@const fromServer = debugInfo.completionTokens > 0}
														{@const tokCount = fromServer
															? debugInfo.completionTokens
															: Math.round(debugInfo.replyChars / 4)}
														{fromServer ? '' : '~'}{(tokCount / genSec).toFixed(1)}
													{:else}
														—
													{/if}
												</span>
											</div>
										</div>
										<div class="debug-trace selectable-content" bind:this={debugTraceEl}>
											{#each debugInfo.trace as entry}
												<div class="trace-entry {entry.kind}">
													<span class="trace-time"
														>+{(
															(entry.time - (debugInfo.requestStart ?? entry.time)) /
															1000
														).toFixed(1)}s</span
													>
													{#if entry.kind === 'model_prompt' || entry.kind === 'intent_prompt'}
														<details class="trace-prompt">
															<summary
																>{entry.kind === 'intent_prompt'
																	? 'Intent classifier prompt'
																	: 'Model request prompt'}</summary
															>
															<pre>{entry.msg}</pre>
														</details>
													{:else if entry.kind === 'tool'}
														<span class="trace-msg trace-tool">🔧 {entry.msg}</span>
													{:else if entry.kind === 'tool_result'}
														<span class="trace-msg trace-tool-result">✅ {entry.msg}</span>
													{:else if entry.kind === 'config'}
														<span class="trace-msg trace-config">⚙️ {entry.msg}</span>
													{:else if entry.kind === 'error'}
														<span class="trace-msg trace-error">❌ {entry.msg}</span>
													{:else if entry.kind === 'gen'}
														<span class="trace-msg trace-gen">💬 {entry.msg}</span>
													{:else if entry.kind === 'done'}
														<span class="trace-msg trace-done">✓ {entry.msg}</span>
													{:else}
														<span class="trace-msg">{entry.msg}</span>
													{/if}
												</div>
											{/each}
										</div>
									</div>
								{/if}
								<div class="prompt-box">
									<textarea
										bind:this={chatTextareaEl}
										bind:value={chatInput}
										onkeydown={(e) => {
											if (e.key === 'Enter' && !e.shiftKey) {
												e.preventDefault();
												if (chatInput.trim() && !isChatStreaming) sendChatMessage();
											}
										}}
										oninput={(e) => {
											const target = e.target as HTMLTextAreaElement;
											target.style.height = 'auto';
											target.style.height = `${Math.min(target.scrollHeight + 2, 150)}px`;
										}}
										placeholder="Ask AI…"
										rows="1"
									></textarea>
									{#if writeTargetNotice && aiInteractionMode === 'operation'}
										<div class="write-target-notice" role="status">
											Place the cursor where you want to write, or select text to rewrite. Then send
											again.
										</div>
									{/if}
									<div class="prompt-toolbar">
										<div class="interaction-mode" role="group" aria-label="AI interaction mode">
											<button
												type="button"
												class:active={aiInteractionMode === 'chat'}
												onclick={() => setAiInteractionMode('chat')}
												title="Chat: answer questions without modifying the note">Chat</button
											>
											<button
												type="button"
												class:active={aiInteractionMode === 'operation'}
												onclick={() => setAiInteractionMode('operation')}
												title="Write: perform the request on the open note">Write</button
											>
										</div>
										<div class="interaction-mode approval-toggle">
											<button
												type="button"
												class:active={!requireToolApproval}
												onclick={() => setToolApproval(!requireToolApproval)}
												title={requireToolApproval
													? 'Ask before each tool action — click to allow automatically'
													: 'Allow tool actions without confirmation — click to ask first'}
											>
												{requireToolApproval ? 'Ask' : 'Allow'}
											</button>
										</div>
										<button
											type="button"
											class="prompt-icon-btn"
											onclick={attachFile}
											title="Attach a file"
											aria-label="Attach a file"
										>
											<svg
												width="16"
												height="16"
												viewBox="0 0 24 24"
												fill="none"
												stroke="currentColor"
												stroke-width="2"
												stroke-linecap="round"
												stroke-linejoin="round"
												><line x1="12" y1="5" x2="12" y2="19" /><line
													x1="5"
													y1="12"
													x2="19"
													y2="12"
												/></svg
											>
										</button>
										{#if armedSelection}
											<button
												type="button"
												class="selection-pill"
												onclick={clearArmedSelection}
												title={armedSelection.cursor
													? `Write target: ${armedSelection.cellIndex === undefined ? 'cursor' : `cell ${armedSelection.cellIndex + 1} cursor`}. Click to clear.`
													: `Use your selection as AI context — ${armedSelection.chars} chars, ${armedSelection.words} word${armedSelection.words === 1 ? '' : 's'}. Click to clear.`}
												aria-label={armedSelection.cursor
													? 'Clear cursor target'
													: `Clear selection (${armedSelection.chars} characters)`}
											>
												<svg
													width="13"
													height="13"
													viewBox="0 0 24 24"
													fill="none"
													stroke="currentColor"
													stroke-width="2"
													stroke-linecap="round"
													stroke-linejoin="round"
													><path
														d="M4 7V5a1 1 0 0 1 1-1h2M17 4h2a1 1 0 0 1 1 1v2M20 17v2a1 1 0 0 1-1 1h-2M7 20H5a1 1 0 0 1-1-1v-2"
													/></svg
												>
												<span>
													{#if armedSelection.cellIndex !== undefined}
														Cell {armedSelection.cellIndex + 1} ·
													{/if}
													{armedSelection.cursor ? 'Cursor' : `${armedSelection.chars} sel`}
												</span>
												<span class="sel-x" aria-hidden="true">✕</span>
											</button>
										{/if}
										<div class="prompt-spacer"></div>
										{#if isChatStreaming}
											<button
												type="button"
												class="send-btn stop-btn"
												onclick={stopChat}
												aria-label="Stop AI"
												title="Stop AI generation"
											>
												<svg
													width="15"
													height="15"
													viewBox="0 0 24 24"
													fill="currentColor"
													aria-hidden="true"
													><rect x="6" y="6" width="12" height="12" rx="1.5" /></svg
												>
											</button>
										{:else}
											<button
												type="button"
												class="send-btn"
												onclick={() => {
													if (chatInput.trim()) sendChatMessage();
												}}
												disabled={!chatInput.trim()}
												aria-label="Send"
												title="Send (Enter)"
											>
												<svg
													width="16"
													height="16"
													viewBox="0 0 24 24"
													fill="none"
													stroke="currentColor"
													stroke-width="2.2"
													stroke-linecap="round"
													stroke-linejoin="round"
													><line x1="12" y1="19" x2="12" y2="5" /><polyline
														points="5 12 12 5 19 12"
													/></svg
												>
											</button>
										{/if}
									</div>
								</div>
							</div>
						</div>
					{:else if activeSidebarTab === 'versions'}
						<div class="versions-container">
							{#if isBusy && noteHistory.length === 0}
								<p class="empty-state">Loading history...</p>
							{:else if noteHistory.length === 0}
								<p class="empty-state">No history found.</p>
							{:else}
								<ul class="history-list">
									{#each noteHistory as commit (commit.hash)}
										<li>
											<div class="commit-header">
												<strong>{commit.message}</strong>
												<span class="commit-date"
													>{new Date(commit.timestamp).toLocaleString()}</span
												>
											</div>
											<div class="commit-actions">
												<button class="secondary" onclick={() => previewVersion(commit.hash)}
													>Preview</button
												>
												<button class="secondary" onclick={() => restoreVersion(commit.hash)}
													>Restore</button
												>
											</div>
										</li>
									{/each}
								</ul>
							{/if}
						</div>
					{/if}
				</div>
			</aside>
		{/if}
	</div>
</div>

<dialog
	bind:this={versionPreviewDialog}
	class="version-preview-dialog"
	onclose={() => (versionPreviewContent = null)}
>
	<div class="dialog-content" style="max-width: 800px; width: 90vw;">
		<h3>Version Preview</h3>
		<div
			class="preview-content"
			style="max-height: 60vh; overflow-y: auto; background: var(--bg-page); padding: 1rem; border-radius: var(--radius-sm); border: 1px solid var(--border-default); white-space: pre-wrap; font-family: var(--font-mono); font-size: 0.875rem; margin: 1rem 0;"
		>
			{versionPreviewContent || 'Loading...'}
		</div>
		<div class="dialog-actions">
			<button class="secondary" onclick={() => versionPreviewDialog?.close()}>Close</button>
			{#if versionPreviewHash}
				<button class="primary" onclick={() => restoreVersion(versionPreviewHash!)}
					>Restore This Version</button
				>
			{/if}
		</div>
	</div>
</dialog>

<dialog
	bind:this={mathDialog}
	class="math-dialog"
	onclose={() => {
		mathValue = '';
		mathError = '';
	}}
>
	<div class="dialog-content">
		<h3>Insert Math</h3>
		<div class="math-container">
			{#if mathLiveReady}
				<svelte:element
					this={'math-field'}
					oninput={(e: any) => (mathValue = e.target.value)}
					style="width: 100%; font-size: 1.5rem; padding: 0.5rem; background: var(--bg-panel); color: var(--text-primary); border: 1px solid var(--border-default); border-radius: var(--radius-xs);"
					>{mathValue}</svelte:element
				>
			{:else}
				<span class="editor-loading">Loading math editor…</span>
			{/if}
		</div>
		{#if mathError}
			<p style="margin: 8px 0 0; font-size: 0.8rem; color: var(--danger, #e5534b);">
				⚠ Won't render in the note: {mathError}
			</p>
		{/if}
		<div class="dialog-actions">
			<button class="secondary" onclick={() => mathDialog?.close()}>Cancel</button>
			<button class="primary" onclick={insertMath} disabled={!mathValue}
				>{mathError ? 'Insert anyway' : 'Insert'}</button
			>
		</div>
	</div>
</dialog>

<dialog
	bind:this={linkNoteDialog}
	class="link-dialog"
	onkeydown={handleLinkSearchKeydown}
	onclose={() => {
		linkSearchQuery = '';
		linkSearchResults = [];
		linkSelectedIndex = 0;
		linkDialogMode = 'notes';
		if (shouldRefocusEditor) refocusEditorSoon();
	}}
>
	<div class="dialog-content">
		{#if linkDialogMode === 'notes'}
			<h3>Link to Note</h3>
			<p style="font-size: 0.875rem; color: var(--text-secondary); margin-bottom: var(--space-4);">
				Search and select a note to link your highlighted text to.
			</p>

			<input
				class="link-search-input"
				bind:value={linkSearchQuery}
				oninput={() => (linkSelectedIndex = 0)}
				use:autofocus
				placeholder="Search notes..."
			/>

			{#if linkSearchQuery.trim() || linkSearchResults.length > 0}
				<div class="link-results-container">
					{#if linkSearchResults.length > 0}
						<ul class="link-results-list">
							{#each linkSearchResults as res, i (res.id + '_' + i)}
								<li>
									<button
										class="link-result-btn"
										class:selected={i === linkSelectedIndex}
										onclick={() => selectNoteForBlocks(res)}
									>
										<strong>{res.title}</strong>
										<span class="folder-badge">{res.folder}</span>
									</button>
								</li>
							{/each}
						</ul>
					{:else if linkSearchQuery.trim()}
						<p class="empty-state">No notes found matching your search.</p>
					{/if}
				</div>
			{/if}
		{:else}
			<h3>Select Block to Reference</h3>
			<p style="font-size: 0.875rem; color: var(--text-secondary); margin-bottom: var(--space-4);">
				Select a specific block from <strong>{selectedNoteForBlocks?.title}</strong> or link the entire
				note.
			</p>

			<input
				class="link-search-input"
				bind:value={linkSearchQuery}
				oninput={() => (linkSelectedIndex = 0)}
				use:autofocus
				placeholder="Search blocks..."
			/>

			<div class="link-results-container">
				{#if filteredBlocks.length > 0}
					<ul class="link-results-list">
						{#each filteredBlocks as block, i}
							<li>
								<button
									class="link-result-btn"
									class:selected={i === linkSelectedIndex}
									onclick={() => insertBlockLink(block)}
								>
									<span
										style={block.isFullNote
											? 'font-weight: bold;'
											: 'font-size: 0.9em; opacity: 0.9; display: -webkit-box; -webkit-line-clamp: 2; -webkit-box-orient: vertical; overflow: hidden;'}
									>
										{block.text}
									</span>
								</button>
							</li>
						{/each}
					</ul>
				{:else}
					<p class="empty-state">No matching blocks found.</p>
				{/if}
			</div>
		{/if}

		<div class="dialog-actions">
			{#if linkDialogMode === 'blocks'}
				<button
					class="secondary"
					style="margin-right: auto;"
					onclick={() => {
						linkDialogMode = 'notes';
						linkSearchQuery = '';
						linkSelectedIndex = 0;
					}}>Back</button
				>
			{/if}
			<button class="secondary" onclick={() => linkNoteDialog?.close()}>Cancel</button>
		</div>
	</div>
</dialog>

<dialog
	bind:this={globalSearchDialog}
	class="link-dialog"
	onkeydown={handleGlobalSearchKeydown}
	onclose={() => {
		globalSearchQuery = '';
		globalSelectedIndex = 0;
		globalBlocks = [];
		if (shouldRefocusEditor) refocusEditorSoon();
	}}
>
	<div class="dialog-content">
		<h3>Search Global Blocks</h3>
		<p style="font-size: 0.875rem; color: var(--text-secondary); margin-bottom: var(--space-4);">
			Search blocks across all notes.
		</p>

		<input
			class="link-search-input"
			bind:value={globalSearchQuery}
			oninput={() => (globalSelectedIndex = 0)}
			placeholder="Search global blocks..."
		/>

		<div class="link-results-container">
			{#if filteredGlobalBlocks.length > 0}
				<ul class="link-results-list">
					{#each filteredGlobalBlocks as block, i}
						<li>
							<button
								class="link-result-btn"
								class:selected={i === globalSelectedIndex}
								onclick={() => insertGlobalBlockLink(block)}
							>
								<div>
									<span
										style="font-size: 0.9em; opacity: 0.9; display: -webkit-box; -webkit-line-clamp: 2; -webkit-box-orient: vertical; overflow: hidden; text-align: left;"
									>
										{block.text}
									</span>
									<span
										style="font-size: 0.7em; opacity: 0.6; display: block; margin-top: 2px; text-align: left;"
									>
										From: {block.sourceNoteTitle}
									</span>
								</div>
							</button>
						</li>
					{/each}
				</ul>
			{:else}
				<p class="empty-state">
					{globalBlocks.length > 0 ? 'No matching blocks found.' : 'Loading blocks...'}
				</p>
			{/if}
		</div>

		<div class="dialog-actions">
			<button class="secondary" onclick={() => globalSearchDialog?.close()}>Cancel</button>
		</div>
	</div>
</dialog>

<dialog
	bind:this={previewNoteDialog}
	class="preview-dialog"
	onclose={() => {
		previewNoteTarget = null;
	}}
>
	{#if previewNoteTarget}
		<div class="preview-layout">
			<div class="preview-main">
				<div class="preview-header">
					<h2>{previewNoteTarget.title}</h2>
					<div class="preview-meta">
						{#if previewNoteTarget.tags.length > 0}
							<span>{previewNoteTarget.tags.join(', ')}</span>
						{/if}
					</div>
				</div>
				<div class="preview-content-scroll">
					<div
						bind:this={previewNoteContainer}
						class="vditor-reset"
						style="padding: 2rem; min-height: 100%;"
					></div>
				</div>
			</div>
			<div class="preview-sidebar">
				<button class="icon-btn" onclick={() => previewNoteDialog?.close()} title="Close Preview">
					<svg
						viewBox="0 0 24 24"
						width="24"
						height="24"
						stroke="currentColor"
						stroke-width="2"
						fill="none"
						stroke-linecap="round"
						stroke-linejoin="round"
						><line x1="18" y1="6" x2="6" y2="18"></line><line x1="6" y1="6" x2="18" y2="18"
						></line></svg
					>
				</button>
				<button class="icon-btn" onclick={expandPreviewNoteDirect} title="Expand Note">
					<svg
						viewBox="0 0 24 24"
						width="24"
						height="24"
						stroke="currentColor"
						stroke-width="2"
						fill="none"
						stroke-linecap="round"
						stroke-linejoin="round"
						><path d="M15 3h6v6"></path><path d="M9 21H3v-6"></path><path d="M21 3l-7 7"
						></path><path d="M3 21l7-7"></path></svg
					>
				</button>
			</div>
		</div>
	{/if}
</dialog>

<dialog bind:this={navigationWarningDialog} class="dialog math-dialog" onclose={cancelNavigation}>
	<div class="dialog-content">
		<h3 style="margin-top: 0;">Unsaved Changes</h3>
		<p style="color: var(--text-secondary); margin-bottom: var(--space-6);">
			The document is currently saving. Are you sure you want to leave? Unsaved changes may be lost.
		</p>
		<div class="dialog-actions">
			<button class="secondary" onclick={cancelNavigation}>Cancel</button>
			<button class="primary" onclick={confirmNavigation}>Leave Page</button>
		</div>
	</div>
</dialog>

<dialog
	bind:this={deleteAttachedNoteDialog}
	class="dialog math-dialog"
	onclose={cancelDeleteAttachedNote}
>
	<div class="dialog-content">
		<h3 style="margin-top: 0;">Delete Attached Note</h3>
		<p style="color: var(--text-secondary); margin-bottom: var(--space-6);">
			All data and annotations in this attached note will be deleted permanently, and the note pane
			will be closed.
		</p>
		<div class="dialog-actions">
			<button class="secondary" onclick={cancelDeleteAttachedNote}>Cancel</button>
			<button class="danger" onclick={confirmDeleteAttachedNote} disabled={isBusy}
				>Delete Note</button
			>
		</div>
	</div>
</dialog>

<dialog bind:this={deleteMainNoteDialog} class="dialog math-dialog">
	<div class="dialog-content">
		<h3 style="margin-top: 0;">Delete Note</h3>
		<p style="color: var(--text-secondary); margin-bottom: var(--space-6);">
			This note will be permanently deleted. This action cannot be undone.
		</p>
		<div class="dialog-actions">
			<button class="secondary" onclick={() => deleteMainNoteDialog?.close()}>Cancel</button>
			<button
				class="danger"
				onclick={() => {
					deleteMainNoteDialog?.close();
					deleteNote();
				}}
				disabled={isBusy}>Delete Note</button
			>
		</div>
	</div>
</dialog>

<dialog bind:this={detachPdfDialog} class="dialog math-dialog">
	<div class="dialog-content">
		<h3 style="margin-top: 0;">Close PDF</h3>
		<p style="color: var(--text-secondary); margin-bottom: var(--space-6);">
			The PDF will be detached from this note. You can re-attach it at any time.
		</p>
		<div class="dialog-actions">
			<button class="secondary" onclick={() => detachPdfDialog?.close()}>Cancel</button>
			<button class="danger" onclick={confirmDetachPdf} disabled={isBusy}>Close PDF</button>
		</div>
	</div>
</dialog>

<dialog
	bind:this={attachPdfDialog}
	class="pdf-attach-dialog"
	onclose={() => {
		pdfSearchQuery = '';
		pdfSelectedIndex = 0;
	}}
>
	<div class="dialog-content">
		<h3>Attach a file</h3>
		<p class="dialog-subtitle">Select a PDF from your workspace or upload a new one.</p>

		<input
			class="link-search-input"
			bind:value={pdfSearchQuery}
			oninput={() => (pdfSelectedIndex = 0)}
			placeholder="Search PDFs..."
		/>

		<div class="pdf-grid-container">
			<button class="pdf-grid-upload-card" onclick={browseAndAttachPdf} disabled={isBusy}>
				<div class="upload-icon-wrapper">
					<svg
						viewBox="0 0 24 24"
						width="32"
						height="32"
						stroke="currentColor"
						stroke-width="1.5"
						fill="none"
						stroke-linecap="round"
						stroke-linejoin="round"
						><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"></path><polyline
							points="17 8 12 3 7 8"
						></polyline><line x1="12" y1="3" x2="12" y2="15"></line></svg
					>
				</div>
				<span class="upload-text">Upload new file</span>
				<span class="upload-subtext">Choose a PDF from your computer</span>
			</button>

			{#each filteredPdfs as pdf, i (pdf.id)}
				<button class="pdf-grid-card" onclick={() => attachPdf(pdf)}>
					<div class="pdf-card-icon">
						<svg
							viewBox="0 0 24 24"
							width="24"
							height="24"
							stroke="var(--accent-300)"
							stroke-width="1.5"
							fill="none"
							stroke-linecap="round"
							stroke-linejoin="round"
							><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"></path><polyline
								points="14 2 14 8 20 8"
							></polyline></svg
						>
					</div>
					<div class="pdf-card-info">
						<strong>{pdf.title}</strong>
						<span>{new Date(pdf.createdAt).toLocaleDateString()}</span>
					</div>
				</button>
			{/each}
		</div>

		<div class="dialog-actions">
			<button class="secondary" onclick={() => attachPdfDialog?.close()}>Cancel</button>
		</div>
	</div>
</dialog>

<style>
	.editor-shell {
		height: 100%;
		position: relative;
		display: grid;
		grid-template-rows: auto 1fr;
		animation: fade-in var(--duration-page) var(--ease-out);
		background: var(--bg-page);
	}

	.editor-shell.resizing,
	.editor-shell.resizing :global(.textLayer) {
		user-select: none !important;
	}

	.editor-header {
		display: flex;
		justify-content: space-between;
		align-items: center;
		padding: var(--space-4) var(--space-6) var(--space-4) var(--space-8);
		border-bottom: 1px solid var(--border-default);
		background: var(--bg-panel-blur);
		backdrop-filter: blur(var(--blur-md));
		position: relative;
		z-index: 1;
	}

	.header-copy {
		display: flex;
		align-items: center;
		gap: var(--space-4);
		flex: 1;
	}

	.back-link,
	input {
		font: inherit;
		font-family: var(--font-mono);
	}

	.back-link {
		border: 1px solid var(--border-subtle);
		border-radius: var(--radius-sm);
		background: transparent;
		color: var(--text-secondary);
		padding: 0.5rem;
		display: flex;
		align-items: center;
		justify-content: center;
		cursor: pointer;
		transition: all var(--duration-fast);
	}
	.back-link:hover {
		color: var(--text-primary);
		border-color: var(--neutral-600);
	}

	.note-loading {
		position: absolute;
		inset: 4.5rem 0 0;
		z-index: 30;
		display: flex;
		align-items: center;
		justify-content: center;
		gap: var(--space-3);
		background: color-mix(in srgb, var(--bg-page) 88%, transparent);
		color: var(--text-secondary);
		font-family: var(--font-mono);
		font-size: 0.75rem;
		text-transform: uppercase;
		letter-spacing: 0.08em;
		pointer-events: none;
	}

	.note-loading-spinner {
		animation: spin 1s linear infinite;
	}

	.status {
		margin: 0;
		font-size: 0.75rem;
		text-transform: uppercase;
		letter-spacing: 0.08em;
		color: var(--text-secondary);
		width: max-content;
	}

	.title-input {
		font-size: 1.5rem;
		font-weight: 600;
		color: var(--text-hero);
		background: transparent;
		border: 1px solid transparent;
		padding: 0.25rem 0.5rem;
		font-family: var(--font-sans);
		flex: 1;
		min-width: 0;
		max-width: none;
	}
	.title-input:hover,
	.title-input:focus {
		border-color: var(--border-subtle);
		background: var(--bg-panel);
	}

	.save-indicator {
		display: flex;
		align-items: center;
		gap: 0.375rem;
		font-size: 0.75rem;
		color: var(--text-secondary);
		font-family: var(--font-mono);
		text-transform: uppercase;
		letter-spacing: 0.05em;
		padding: 0.25rem 0.5rem;
		margin-left: auto;
	}

	.save-indicator.saving {
		color: var(--accent-100);
	}

	.save-indicator .dot {
		width: 6px;
		height: 6px;
		border-radius: 50%;
		background: var(--neutral-400);
	}

	.spinner {
		animation: spin 1s linear infinite;
	}

	@keyframes spin {
		100% {
			transform: rotate(360deg);
		}
	}
	button:disabled {
		opacity: 0.6;
		cursor: not-allowed;
	}

	.main-layout {
		flex: 1;
		min-height: 0;
		position: relative;
		display: flex;
		overflow: hidden;
		z-index: 20; /* Ensures tooltips render above the header's stacking context */
	}

	.main-layout.split-layout {
		overflow-x: auto;
	}

	.pdf-pane {
		min-width: 26rem;
	}

	.pdf-index-status {
		position: absolute;
		right: 0.75rem;
		bottom: 0.75rem;
		z-index: 8;
		padding: 0.35rem 0.55rem;
		border: 1px solid var(--border-default);
		border-radius: var(--radius-sm);
		background: var(--bg-panel);
		color: var(--text-secondary);
		font-size: 0.72rem;
		pointer-events: none;
	}

	.pdf-index-status.error {
		color: var(--danger-text);
		border-color: var(--danger-border);
	}

	/* In the .tex split (PDF preview + editor), let both panes shrink with the
	   window. Otherwise main-pane's 800px min overflows the overflow:hidden layout
	   on small windows, pushing the editor's horizontal scrollbar off-screen and
	   clipping long lines. With min-width:0 the inner editor scroller handles them. */
	.main-layout .main-pane.tex-pane {
		min-width: 0;
	}
	/* Keep enough width for the PDF toolbar so its buttons don't crowd/overlap,
	   but small enough that the editor still gets room to shrink + scroll. */
	.main-layout .pdf-pane.tex-pane {
		min-width: 15rem;
	}

	/* Floating label over the compiled-PDF pane (tex split) explaining the layout. */
	.tex-pane-badge {
		position: absolute;
		top: 8px;
		left: 50%;
		transform: translateX(-50%);
		z-index: 6;
		pointer-events: none;
		max-width: 92%;
		text-align: center;
		background: var(--bg-panel);
		color: var(--text-secondary);
		border: 1px solid var(--border-default);
		border-radius: 6px;
		padding: 3px 10px;
		font-size: 0.72rem;
		box-shadow: 0 2px 8px var(--shadow-color, rgba(0, 0, 0, 0.15));
	}

	.resizer {
		width: 10px;
		flex: 0 0 10px;
		cursor: col-resize;
		position: relative;
		background: linear-gradient(
			90deg,
			transparent 0,
			transparent 3px,
			var(--border-subtle) 3px,
			var(--border-subtle) 7px,
			transparent 7px
		);
		transition: background 0.2s ease;
	}

	.resizer:hover,
	.resizer.resizing {
		background: linear-gradient(
			90deg,
			transparent 0,
			transparent 2px,
			var(--accent-100) 2px,
			var(--accent-100) 8px,
			transparent 8px
		);
	}

	.sidebar-backdrop {
		display: none;
	}

	/* Main Pane */
	.main-pane {
		flex: 1;
		min-width: 26rem;
		display: flex;
		flex-direction: column;
		background: var(--bg-page);
		align-items: stretch;
		min-height: 0;
		overflow-y: auto; /* Make main pane the scroll container */
		overflow-x: hidden;
	}

	.danger {
		border: 1px solid var(--danger-border);
		background: var(--danger-bg);
		color: var(--danger-text);
	}

	.danger:hover:not(:disabled) {
		background: var(--danger-bg-strong);
		color: var(--danger-text);
	}

	.content-area {
		width: 100%;
		min-width: 0;
		display: flex;
		flex-direction: column;
		flex: 1;
		min-height: 0;
	}

	.vditor-wrapper {
		position: relative;
		border: none !important;
		flex: 1;
		min-width: 0;
		min-height: 0;
	}

	.tools-loading-overlay {
		position: absolute;
		inset: 0;
		z-index: 20;
		display: flex;
		align-items: center;
		justify-content: center;
		background: color-mix(in srgb, var(--bg-primary) 88%, transparent);
		color: var(--text-muted);
		font-size: 0.8rem;
		pointer-events: auto;
		cursor: wait;
	}

	.vditor-wrapper.tools-loading :global(.vditor-toolbar) {
		opacity: 0.35;
		filter: grayscale(1);
		pointer-events: none;
	}

	:global(.vditor) {
		height: 100% !important;
	}

	:global(.vditor-reset) {
		padding-top: var(--space-6) !important;
	}

	.toolbar-note-actions-container {
		position: absolute;
		top: 0;
		right: var(--space-6);
		height: 48px;
		display: flex;
		align-items: center;
		justify-content: center;
		gap: var(--space-2);
		z-index: 40;
	}

	.toolbar-close-note-btn {
		pointer-events: auto;
		border: 1px solid var(--border-subtle);
		background: var(--bg-panel);
		color: currentColor;
		border-radius: var(--radius-sm);
		padding: 0.4rem 0.75rem;
		height: 32px;
		font-size: 0.82rem;
		font-family: var(--font-mono);
		line-height: 1;
		white-space: nowrap;
	}

	.toolbar-close-note-btn:hover:not(:disabled) {
		background: var(--danger-bg-strong);
		color: var(--danger-text);
	}

	.toolbar-attach-pdf-btn {
		pointer-events: auto;
		display: flex;
		align-items: center;
		gap: 0.375rem;
		border: 1px solid var(--border-subtle);
		background: var(--bg-panel);
		color: var(--text-secondary);
		border-radius: var(--radius-sm);
		padding: 0.4rem 0.75rem;
		height: 32px;
		font-size: 0.82rem;
		font-family: var(--font-mono);
		line-height: 1;
		white-space: nowrap;
		cursor: pointer;
		transition: all var(--duration-fast);
	}

	.toolbar-attach-pdf-btn:hover:not(:disabled) {
		border-color: var(--accent-200);
		color: var(--accent-100);
	}

	.toolbar-overlay-toggle {
		width: 28px;
		height: 28px;
		display: flex;
		align-items: center;
		justify-content: center;
		background: var(--bg-surface);
		border: 1px solid var(--border-subtle);
		border-radius: var(--radius-sm);
		color: var(--text-secondary);
		cursor: pointer;
		transition: all 0.2s;
	}
	.toolbar-overlay-toggle:hover {
		color: var(--text-primary);
		background: var(--hover-overlay);
	}
	.toolbar-overlay-toggle.expanded svg {
		transform: rotate(180deg);
	}

	:global(.vditor-wrapper:not(.toolbar-expanded) .vditor-toolbar) {
		max-height: 48px;
		overflow: visible !important;
	}

	/* Force all upward-facing tooltips (__n, __ne, __nw) to point downwards vertically */
	:global(.vditor-toolbar .vditor-tooltipped__n::after),
	:global(.vditor-toolbar .vditor-tooltipped__ne::after),
	:global(.vditor-toolbar .vditor-tooltipped__nw::after) {
		bottom: auto !important;
		top: 100% !important;
		margin-bottom: 0 !important;
		margin-top: 5px !important;
	}

	:global(.vditor-toolbar .vditor-tooltipped__n::before),
	:global(.vditor-toolbar .vditor-tooltipped__ne::before),
	:global(.vditor-toolbar .vditor-tooltipped__nw::before) {
		top: auto !important;
		bottom: -5px !important;
		border-top-color: transparent !important;
		border-bottom-color: var(--neutral-800) !important;
	}

	:global(.vditor) {
		border: none !important;
		overflow: visible !important;
		min-width: 0 !important;
		height: 100% !important;
		display: flex !important;
		flex-direction: column !important;
		--panel-background-color: var(--bg-page) !important;
		--textarea-background-color: var(--bg-page) !important;
		--toolbar-background-color: var(--bg-panel-blur) !important;
	}

	:global(.vditor-content) {
		display: flex !important;
		flex-direction: column !important;
		align-items: stretch !important;
		width: 100% !important;
		min-width: 0 !important;
		background: var(--bg-page) !important;
		flex: 1 !important;
		min-height: 0 !important;
		overflow: hidden !important;
	}

	:global(.vditor-ir),
	:global(.vditor-sv),
	:global(.vditor-preview) {
		width: 100% !important;
		box-sizing: border-box !important;
		flex: 1 !important;
		min-height: 0 !important;
		overflow-y: auto !important;
	}

	/* Clear padding from the scroll container so its scrollbar is pinned to the far right edge */
	:global(.vditor-ir) {
		padding: 0 !important;
	}

	/* Hide the middle scrollbar in Split View (left pane) */
	:global(.vditor-sv)::-webkit-scrollbar {
		display: none !important;
		width: 0 !important;
		background: transparent !important;
	}
	:global(.vditor-sv) {
		scrollbar-width: none !important;
		-ms-overflow-style: none !important;
	}

	/* The editor's usable text width is exactly 120 monospace characters. */
	:global(.vditor-reset) {
		width: min(100%, calc(120ch + 2 * var(--space-8))) !important;
		max-width: none !important;
		margin: 0 auto !important;
		padding-left: var(--space-8) !important;
		padding-right: var(--space-8) !important;
		white-space: pre-wrap !important;
		overflow-wrap: anywhere !important;
		word-break: break-word !important;
		overflow-x: hidden !important;
		box-sizing: border-box !important;
	}

	:global(.vditor-toolbar) {
		min-width: 0 !important;
		flex-wrap: wrap !important;
	}

	:global(.vditor-reset *) {
		max-width: 100%;
		overflow-wrap: anywhere;
		word-break: break-word;
	}

	/* Keep Markdown editing and preview readable in narrow panes. */
	:global(.vditor-reset),
	:global(.vditor-textarea) {
		font-size: 12px !important;
	}

	:global(.vditor-ir) {
		min-width: 0 !important;
		overflow-x: hidden !important;
	}

	:global(.vditor-preview__action) {
		display: none !important;
	}

	@media (min-width: 1200px) {
		:global(.vditor-content:has(.vditor-sv[style*='block'])) {
			flex-direction: row !important;
			align-items: stretch !important;
			justify-content: center !important;
			gap: 0 !important;
			padding: 0 !important;
		}

		:global(.vditor-content:has(.vditor-sv[style*='block']) .vditor-ir),
		:global(.vditor-content:has(.vditor-sv[style*='block']) .vditor-sv),
		:global(.vditor-content:has(.vditor-sv[style*='block']) .vditor-preview) {
			margin: 0 !important;
		}

		:global(.vditor-content:has(.vditor-sv[style*='block']) .vditor-reset) {
			padding-left: var(--space-6) !important;
			padding-right: var(--space-6) !important;
		}
	}

	:global(.vditor-reset),
	:global(.vditor-textarea) {
		font-family: var(--font-mono) !important;
	}

	:global(.vditor-ir),
	:global(.vditor-reset) {
		color: var(--text-primary) !important;
	}

	:global(.vditor-toolbar) {
		border-bottom: 1px solid var(--border-subtle) !important;
		padding: var(--space-2) var(--space-4) !important;
		padding-right: 120px !important;
		transition: max-height 0.2s ease-out;
		position: relative !important;
		z-index: 30 !important;
	}

	:global(.vditor-wrapper.has-pdf-note .vditor-toolbar) {
		padding-right: 120px !important;
	}

	/* Sidebar (Mobile / Overlay mode by default) */
	.sidebar {
		position: absolute;
		top: 0;
		right: 0;
		bottom: 0;
		width: var(--sidebar-width, 20rem);
		max-width: 85vw;
		background: var(--bg-panel);
		padding: 0 var(--space-6) var(--space-6) var(--space-6);
		display: flex;
		flex-direction: column;
		gap: var(--space-6);
		overflow-y: auto;
		z-index: 100;
		transform: translateX(100%);
		transition:
			transform 0.3s cubic-bezier(0.16, 1, 0.3, 1),
			margin-right 0.3s cubic-bezier(0.16, 1, 0.3, 1);
		border-left: 1px solid var(--border-default);
		border-radius: 0 !important;
		box-shadow: -4px 0 24px var(--shadow-color);
		font-family: var(--font-mono);
	}

	.sidebar.open {
		transform: translateX(0);
	}

	.sidebar-backdrop {
		position: absolute;
		inset: 0;
		background: var(--scrim-soft);
		backdrop-filter: blur(var(--blur-sm));
		z-index: 90;
		animation: fade-in var(--duration-fast) ease-out;
	}

	/* Large Screen — sidebar docks side by side with the editor */
	@media (min-width: 1380px) {
		.sidebar {
			position: relative;
			transform: none;
			margin-right: calc(var(--sidebar-width, 20rem) * -1);
			/* Docking is allowed only when the editor can retain its pane minimum. */
			max-width: calc(100% - 26rem);
			box-shadow: none;
			flex-shrink: 0;
		}

		.sidebar.open {
			transform: none;
			margin-right: 0;
		}

		.sidebar-backdrop {
			display: none !important;
		}
	}

	.sidebar-resizer {
		position: absolute;
		left: -3px;
		top: 0;
		bottom: 0;
		width: 6px;
		cursor: ew-resize;
		z-index: 1000;
		transition: background 0.2s ease;
	}
	.sidebar-resizer:hover,
	.sidebar-resizer.resizing {
		background: var(--accent-100);
	}

	.sidebar-section {
		display: flex;
		flex-direction: column;
		gap: var(--space-3);
	}

	.latex-preview-section {
		border-top: 1px solid var(--border-default);
		padding-top: var(--space-4);
	}

	.latex-preview-controls {
		display: flex;
		gap: var(--space-2);
	}

	.latex-preview-controls button {
		font-size: 0.75rem;
		white-space: nowrap;
	}

	.latex-preview-state {
		display: flex;
		align-items: flex-start;
		gap: var(--space-2);
		font-size: 0.75rem;
		color: var(--text-secondary);
		line-height: 1.4;
	}

	.latex-preview-state.error,
	.latex-error,
	.latex-diagnostics {
		color: var(--danger-400, #f87171);
	}

	.latex-status-dot {
		width: 0.45rem;
		height: 0.45rem;
		margin-top: 0.3rem;
		border-radius: 50%;
		background: var(--neutral-500);
		flex: 0 0 auto;
	}

	.latex-status-dot.active {
		background: var(--success-400, #4ade80);
	}
	.latex-status-dot.pending {
		background: var(--accent-200);
	}

	.latex-error,
	.latex-diagnostics {
		margin: 0;
		font-size: 0.72rem;
		line-height: 1.45;
		word-break: break-word;
	}

	.latex-diagnostics {
		padding-left: 1.1rem;
	}

	.latex-diagnostics span {
		font-family: var(--font-mono);
		font-size: 0.68rem;
	}

	.sidebar h3 {
		margin: 0;
		font-size: 0.75rem;
		text-transform: uppercase;
		letter-spacing: 0.08em;
		color: var(--text-primary);
	}

	.row-badge {
		border: 1px solid var(--border-default);
		border-radius: 4px;
		padding: 2px 6px;
		font-size: 0.65rem;
		color: var(--neutral-400);
		font-family: var(--font-mono);
		background: var(--overlay-faint);
		flex-shrink: 0;
		white-space: nowrap;
	}

	.tag-input {
		width: 100%;
		border: 1px solid var(--border-default);
		border-radius: var(--radius-xs);
		background: var(--bg-page);
		padding: 0.625rem 0.75rem;
		color: var(--text-primary);
		outline: none;
	}
	.tag-input:focus {
		border-color: var(--accent-200);
	}

	.empty-state {
		margin: 0;
		font-size: 0.875rem;
		color: var(--neutral-500);
		font-style: italic;
	}

	.related-list {
		list-style: none;
		padding: 0;
		margin: 0;
		display: flex;
		flex-direction: column;
		gap: var(--space-2);
	}

	.related-list a {
		color: var(--text-primary);
		text-decoration: none;
		font-size: 0.875rem;
		display: block;
		padding: 0.375rem 0;
		border-bottom: 1px solid transparent;
		transition: color var(--duration-fast);
	}
	.related-list a:hover {
		color: var(--accent-100);
	}

	@keyframes fade-in {
		from {
			opacity: 0;
			transform: translateY(8px);
		}
		to {
			opacity: 1;
			transform: translateY(0);
		}
	}

	@media (max-width: 1024px) {
		.editor-header {
			flex-wrap: wrap;
			gap: var(--space-4);
			position: sticky;
			top: 0;
			z-index: 10;
		}
		.title-input {
			max-width: 100%;
		}
	}

	.math-dialog {
		padding: 0;
		border: 1px solid var(--border-default);
		border-radius: var(--radius-sm);
		background: var(--bg-panel);
		color: var(--text-primary);
		max-width: 40rem;
		width: 100%;
		backdrop-filter: blur(var(--blur-md));
		box-shadow: none;
	}
	.math-dialog::backdrop {
		background: var(--scrim);
		backdrop-filter: blur(var(--blur-sm));
	}
	.dialog-content {
		padding: var(--space-6);
		display: grid;
		gap: var(--space-4);
	}
	.dialog-content h3 {
		margin: 0;
		font-size: 1.25rem;
		color: var(--text-hero);
	}
	.math-container {
		min-height: 4rem;
	}
	.dialog-actions {
		display: flex;
		justify-content: flex-end;
		gap: var(--space-2);
		margin-top: var(--space-4);
	}
	.dialog-actions button {
		padding: 0.625rem 1rem;
		border-radius: var(--radius-sm);
		border: 1px solid var(--border-default);
		background: var(--bg-panel);
		color: var(--text-primary);
		cursor: pointer;
	}
	.dialog-actions .primary {
		background: var(--accent-200);
		color: var(--text-inverse);
		border-color: var(--accent-200);
	}

	.link-dialog {
		padding: 0;
		border: 1px solid var(--border-default);
		border-radius: var(--radius-sm);
		background: var(--bg-panel);
		color: var(--text-primary);
		max-width: 40rem;
		width: 100%;
		backdrop-filter: blur(var(--blur-md));
		outline: none;
		box-shadow: none;
	}
	.link-dialog::backdrop {
		background: var(--scrim);
		backdrop-filter: blur(var(--blur-sm));
	}
	.link-search-input {
		width: 100%;
		border: 2px solid var(--border-default);
		border-radius: var(--radius-sm);
		background: var(--bg-panel);
		padding: 1rem 1.25rem;
		font-size: 1.125rem;
		color: var(--text-primary);
		outline: none;
		font-family: var(--font-sans);
		margin-bottom: var(--space-4);
		transition: border-color 0.2s;
	}
	.link-search-input:focus {
		border-color: var(--accent-200);
	}

	.link-results-container {
		max-height: 300px;
		overflow-y: auto;
		border: 1px solid var(--border-subtle);
		border-radius: var(--radius-xs);
		background: var(--bg-panel);
		padding: var(--space-2);
	}

	.link-results-list {
		list-style: none;
		margin: 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: 2px;
	}

	.link-result-btn {
		width: 100%;
		display: flex;
		justify-content: space-between;
		align-items: center;
		padding: 0.5rem 0.75rem;
		background: transparent;
		border: none;
		border-radius: var(--radius-xs);
		color: var(--text-primary);
		cursor: pointer;
		text-align: left;
		transition: background 0.1s;
	}
	.link-result-btn:hover,
	.link-result-btn.selected {
		background: var(--accent-tint);
	}

	.folder-badge {
		font-size: 0.7rem;
		color: var(--text-secondary);
		background: var(--bg-page);
		padding: 0.125rem 0.375rem;
		border-radius: 1rem;
		border: 1px solid var(--border-subtle);
	}

	:global(.has-attached-file [data-type='attach-pdf']) {
		opacity: 0.3 !important;
		pointer-events: none !important;
		cursor: not-allowed !important;
	}

	.pdf-attach-dialog {
		width: 100%;
		max-width: 800px;
		background: var(--bg-modal);
		border: 1px solid var(--border-subtle);
		border-radius: var(--radius-md);
		color: var(--text-primary);
		box-shadow: 0 10px 30px var(--shadow-color-strong);
	}
	.pdf-attach-dialog::backdrop {
		background: var(--scrim);
		backdrop-filter: blur(2px);
	}
	.pdf-attach-dialog .dialog-content {
		padding: var(--space-6);
	}
	.pdf-attach-dialog h3 {
		margin-top: 0;
		font-size: 1.25rem;
		font-weight: 500;
	}
	.dialog-subtitle {
		font-size: 0.875rem;
		color: var(--text-secondary);
		margin-bottom: var(--space-4);
	}

	.pdf-grid-container {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(180px, 1fr));
		gap: 16px;
		margin-top: 12px;
		margin-bottom: 24px;
		max-height: 400px;
		overflow-y: auto;
		padding: 4px;
		padding-right: 8px;
	}

	.pdf-grid-container::-webkit-scrollbar {
		width: 6px;
	}
	.pdf-grid-container::-webkit-scrollbar-thumb {
		background: var(--border-default);
		border-radius: 4px;
	}

	.pdf-grid-upload-card {
		background: var(--overlay-faint);
		border: 1px dashed var(--border-default);
		border-radius: var(--radius-sm);
		padding: 24px 16px;
		display: flex;
		flex-direction: column;
		align-items: center;
		justify-content: center;
		gap: 8px;
		cursor: pointer;
		transition: all 0.2s ease;
		color: var(--text-primary);
		text-align: center;
		font-family: inherit;
	}
	.pdf-grid-upload-card:hover:not(:disabled) {
		background: var(--hover-overlay);
		border-color: var(--accent-300);
	}
	.pdf-grid-upload-card:disabled {
		opacity: 0.5;
		cursor: not-allowed;
	}
	.upload-icon-wrapper {
		color: var(--accent-300);
		margin-bottom: 4px;
	}
	.upload-text {
		font-weight: 500;
		font-size: 0.95rem;
	}
	.upload-subtext {
		font-size: 0.75rem;
		color: var(--text-secondary);
	}

	.pdf-grid-card {
		background: var(--overlay-faint);
		border: 1px solid var(--border-subtle);
		border-radius: var(--radius-sm);
		padding: 16px;
		display: flex;
		flex-direction: column;
		align-items: flex-start;
		gap: 12px;
		cursor: pointer;
		transition: all 0.2s ease;
		text-align: left;
		font-family: inherit;
		color: inherit;
	}
	.pdf-grid-card:hover {
		background: var(--hover-overlay-strong);
		border-color: var(--border-default);
		transform: translateY(-2px);
	}
	.pdf-card-icon {
		background: var(--bg-code);
		padding: 12px;
		border-radius: 8px;
		display: flex;
		align-items: center;
		justify-content: center;
	}
	.pdf-card-info {
		display: flex;
		flex-direction: column;
		gap: 4px;
		width: 100%;
	}
	.pdf-card-info strong {
		font-size: 0.9rem;
		font-weight: 500;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
		width: 100%;
	}
	.pdf-card-info span {
		font-size: 0.75rem;
		color: var(--text-secondary);
	}

	.preview-dialog {
		padding: 0;
		border: none;
		border-radius: var(--radius-md);
		background: transparent;
		color: var(--text-primary);
		width: 800px;
		max-width: 90vw;
		height: 75vh;
		max-height: 80vh;
		outline: none;
	}
	.preview-dialog::backdrop {
		background: var(--scrim-soft);
		backdrop-filter: blur(var(--blur-sm));
	}
	.preview-layout {
		display: flex;
		height: 100%;
		gap: var(--space-4);
		position: relative;
	}
	.preview-main {
		flex: 1;
		background: var(--bg-page);
		border-radius: var(--radius-md);
		border: 1px solid var(--border-default);
		display: flex;
		flex-direction: column;
		overflow: hidden;
		box-shadow: 0 12px 48px var(--shadow-color-strong);
	}
	.preview-header {
		padding: var(--space-6) var(--space-8);
		border-bottom: 1px solid var(--border-subtle);
		background: var(--bg-panel);
	}
	.preview-header h2 {
		margin: 0 0 var(--space-2) 0;
		font-size: 1.5rem;
		color: var(--text-hero);
	}
	.preview-meta span {
		font-family: var(--font-mono);
		font-size: 0.875rem;
		color: var(--text-secondary);
		background: var(--neutral-600);
		padding: 0.1rem 0.4rem;
		border-radius: var(--radius-xs);
	}
	.preview-content-scroll {
		flex: 1;
		overflow-y: auto;
		background: var(--bg-page);
	}
	.preview-sidebar {
		display: flex;
		flex-direction: column;
		gap: var(--space-3);
		padding-top: var(--space-4);
		align-items: center;
	}
	.icon-btn {
		width: 48px;
		height: 48px;
		border-radius: 50%;
		background: var(--bg-panel);
		border: 1px solid var(--border-default);
		color: var(--text-primary);
		display: flex;
		align-items: center;
		justify-content: center;
		cursor: pointer;
		transition: all 0.2s;
	}
	.icon-btn:hover {
		background: var(--neutral-600);
		color: var(--text-inverse);
	}

	/* Base State (Collapsed): Hide the link text completely to make the transclusion look seamless */
	:global(
		.transclusion-wrapper[data-block-content]:not([data-block-content='']):not(
				.vditor-ir__node--expand
			):not(.force-expand)
			.vditor-ir__link
	) {
		display: none !important;
	}
	/* Strip the wrapper pill background when collapsed because we only want the ::after to show */
	:global(
		.transclusion-wrapper[data-block-content]:not([data-block-content='']):not(
				.vditor-ir__node--expand
			):not(.force-expand)
	) {
		padding: 0 !important;
		background: transparent !important;
		border: none !important;
		display: block !important;
	}
	/* Ensure the ::after preview has no top margin since there's no text above it */
	:global(
		.transclusion-wrapper[data-block-content]:not([data-block-content='']):not(
				.vditor-ir__node--expand
			):not(.force-expand)::after
	) {
		margin-top: 0 !important;
	}

	/* Active State (Selected or Edited): Restore the orange pill styling */
	:global(.transclusion-wrapper.force-expand),
	:global(.transclusion-wrapper.vditor-ir__node--expand) {
		padding: 0.25rem 0.5rem !important;
		background: var(--accent-tint) !important;
		border-left: 3px solid var(--accent-200) !important;
		border-radius: 0 var(--radius-sm) var(--radius-sm) 0 !important;
		display: inline-block !important;
	}

	/* Style the link text nicely when active */
	:global(.transclusion-wrapper .vditor-ir__link) {
		color: var(--accent-200) !important;
		font-family: var(--font-mono) !important;
		font-size: 0.875em !important;
	}

	/* Render the block content seamlessly via pseudo-element */
	:global(.transclusion-wrapper::after) {
		content: attr(data-block-content);
		display: block;
		margin-top: 0.5rem;
		padding: var(--space-3) 1rem;
		background: var(--accent-tint);
		border-left: 3px solid var(--accent-200);
		border-radius: 0 var(--radius-sm) var(--radius-sm) 0;
		color: var(--text-secondary);
		font-family: var(--font-mono);
		white-space: pre-wrap;
		font-size: 0.85em;
		line-height: 1.5;
		cursor: default;
	}

	/* Hide the transclusion content when the link is actively selected or edited to prevent visual clutter */
	:global(.transclusion-wrapper.force-expand::after),
	:global(.transclusion-wrapper.vditor-ir__node--expand::after) {
		display: none !important;
	}

	/* Prevent Vditor from "truncating" (hiding) link markers ONLY when actively selected or edited */
	:global(.vditor-ir__node[data-type='a'].force-expand .vditor-ir__marker),
	:global(.vditor-ir__node[data-type='a'].vditor-ir__node--expand .vditor-ir__marker) {
		display: inline !important;
		opacity: 0.6;
		font-family: var(--font-mono);
	}

	/* Vditor link theme override */
	:global(.vditor-reset a),
	:global(.vditor-ir__link) {
		color: var(--accent-200) !important;
		text-decoration-color: var(--accent-200) !important;
	}

	/* Ensure Vditor fullscreen covers the custom titlebar */
	:global(.vditor--fullscreen) {
		z-index: 10000 !important;
	}

	/* Fullscreen Indicator */
	.fullscreen-indicator {
		display: none;
		position: fixed;
		bottom: var(--space-8);
		right: var(--space-8);
		background: var(--bg-panel-blur);
		color: var(--text-secondary);
		padding: var(--space-2) var(--space-4);
		border-radius: var(--radius-full);
		font-size: 0.875rem;
		pointer-events: none;
		z-index: 10001; /* Must be above Vditor's 10000 */
		backdrop-filter: blur(var(--blur-md));
		border: 1px solid var(--border-default);
		box-shadow: var(--shadow-lg);
	}

	.fullscreen-indicator span {
		background: var(--bg-panel);
		color: var(--text-primary);
		padding: 2px 6px;
		border-radius: var(--radius-xs);
		border: 1px solid var(--border-subtle);
		font-family: var(--font-mono);
		font-size: 0.75rem;
	}

	:global(.content-area:has(.vditor--fullscreen)) .fullscreen-indicator {
		display: flex;
		align-items: center;
		gap: var(--space-2);
		animation: fade-in 0.3s ease-out;
	}

	:global(.vditor-hint button[data-mode='wysiwyg']) {
		display: none !important;
	}

	/* Sidebar Tabs */
	.sidebar-tabs {
		display: flex;
		height: 48px;
		border-bottom: 1px solid var(--border-subtle);
		margin-bottom: var(--space-4);
		flex-shrink: 0;
	}
	.sidebar-tabs button {
		flex: 1;
		background: transparent;
		border: none;
		color: var(--text-secondary);
		padding: 0;
		font-family: var(--font-sans);
		font-size: 0.875rem;
		cursor: pointer;
		border-bottom: 2px solid transparent;
		transition: all var(--duration-fast);
		display: flex;
		align-items: center;
		justify-content: center;
	}
	.sidebar-tabs button.active {
		color: var(--accent-100);
		border-bottom-color: var(--accent-100);
	}
	.sidebar-tabs button:hover:not(.active) {
		color: var(--text-primary);
	}

	.sidebar-content {
		display: flex;
		flex-direction: column;
		gap: var(--space-6);
		flex: 1;
		min-height: 0;
		overflow-y: auto;
	}

	.sidebar-content::-webkit-scrollbar,
	.chat-messages::-webkit-scrollbar,
	.versions-container::-webkit-scrollbar,
	.sidebar::-webkit-scrollbar {
		display: none;
	}
	.sidebar-content,
	.chat-messages,
	.versions-container,
	.sidebar {
		-ms-overflow-style: none;
		scrollbar-width: none;
	}

	/* Chat UI */
	.chat-container {
		display: flex;
		flex-direction: column;
		height: 100%;
		flex: 1;
	}
	.chat-messages {
		flex: 1;
		overflow-y: auto;
		display: flex;
		flex-direction: column;
		gap: var(--space-4);
		padding-bottom: var(--space-4);
	}
	.chat-message {
		display: flex;
		flex-direction: column;
	}
	.chat-message.tool-only {
		margin-top: calc(-1 * var(--space-3));
		margin-bottom: calc(-1 * var(--space-3));
	}
	.chat-message.user {
		align-items: flex-end;
	}
	.chat-message.assistant {
		align-items: flex-start;
	}
	.chat-bubble {
		max-width: 85%;
		padding: var(--space-3) var(--space-4);
		border-radius: var(--radius-md);
		font-size: 0.875rem;
		line-height: 1.5;
		min-width: 0;
		word-break: break-word;
		overflow-wrap: anywhere;
	}
	.chat-message.user .chat-bubble {
		background: var(--accent-200);
		color: var(--on-accent);
	}
	.chat-message.assistant .chat-bubble {
		background: var(--bg-panel);
		color: var(--text-primary);
	}
	.chat-message.tool-only .chat-bubble {
		padding-top: var(--space-1);
		padding-bottom: var(--space-1);
		background: transparent;
	}
	.chat-bubble.error {
		background: color-mix(in srgb, var(--bg-panel) 82%, var(--accent-200));
	}
	.chat-bubble.error {
		border-left: 3px solid var(--danger);
		background-color: var(--danger-bg);
	}

	.approval-card {
		background: var(--bg-code);
		border: 1px solid var(--accent-200);
		border-radius: var(--radius-md);
		padding: var(--space-3);
		margin: var(--space-2) 0;
		display: flex;
		flex-direction: column;
		gap: var(--space-2);
		width: 100%;
		box-sizing: border-box;
	}
	.approval-card.rejected {
		border-color: var(--danger-border);
		background: var(--danger-bg);
	}
	.approval-card.rejected .title {
		color: var(--danger-text);
	}

	.approval-card .title {
		margin: 0;
		font-size: 0.85rem;
		color: var(--accent-300);
	}

	.approval-card pre {
		background: var(--bg-code);
		padding: var(--space-2);
		border-radius: var(--radius-sm);
		font-size: 0.75rem;
		font-family: var(--font-mono);
		white-space: pre-wrap;
		word-break: break-word;
		max-height: 120px;
		overflow-y: auto;
		margin: 0;
		color: var(--text-muted);
	}

	.approval-card pre::-webkit-scrollbar {
		width: 6px;
		height: 6px;
	}
	.approval-card pre::-webkit-scrollbar-track {
		background: transparent;
	}
	.approval-card pre::-webkit-scrollbar-thumb {
		background: var(--neutral-700, #444);
		border-radius: 3px;
	}
	.approval-card pre::-webkit-scrollbar-thumb:hover {
		background: var(--neutral-500, #666);
	}

	.approval-actions {
		display: flex;
		gap: var(--space-2);
		flex-wrap: wrap; /* Fix responsiveness for narrow sidebars */
	}

	.pending-approval-bar {
		display: flex;
		justify-content: space-between;
		align-items: center;
		padding: var(--space-3) var(--space-4);
		background: var(--surface-2);
		border-top: 1px solid var(--border-subtle);
		border-bottom: 1px solid var(--border-subtle);
		gap: var(--space-4);
	}
	.pending-info {
		display: flex;
		align-items: center;
		gap: var(--space-2);
		font-size: 0.85rem;
		color: var(--text-primary);
	}
	.pending-actions {
		display: flex;
		gap: var(--space-2);
	}
	.pending-actions button {
		padding: 0.4rem 0.8rem;
		border-radius: var(--radius-sm);
		font-size: 0.8rem;
		cursor: pointer;
		font-weight: 500;
	}
	.pending-actions .primary {
		background: var(--accent-500);
		color: white;
		border: 1px solid var(--accent-500);
	}
	.pending-actions .secondary {
		background: transparent;
		color: var(--text-primary);
		border: 1px solid var(--border-default);
	}
	.chat-tools {
		display: flex;
		flex-direction: column;
		gap: var(--space-2);
		margin-bottom: var(--space-2);
	}
	.chat-tools.streaming {
		position: sticky;
		top: 0;
		z-index: 1;
		background: inherit;
		padding-bottom: var(--space-2);
	}
	.chat-input-area {
		display: flex;
		flex-direction: column;
		gap: var(--space-2);
		padding-top: var(--space-3);
		border-top: 1px solid var(--border-subtle);
	}
	.prompt-box {
		background: var(--neutral-1000);
		border: 1px solid var(--border-default);
		border-radius: var(--radius-md);
		padding: var(--space-2);
		display: flex;
		flex-direction: column;
		gap: 2px;
		transition:
			border-color 0.15s ease,
			box-shadow 0.15s ease;
	}
	.prompt-box:focus-within {
		border-color: color-mix(in srgb, var(--accent-200) 55%, var(--border-default));
		box-shadow: 0 0 0 3px color-mix(in srgb, var(--accent-200) 12%, transparent);
	}
	.chat-input-area textarea {
		width: 100%;
		background: transparent;
		border: none;
		border-radius: 0;
		padding: var(--space-1) var(--space-2);
		color: var(--text-primary);
		outline: none;
		resize: none;
		font-family: inherit;
		font-size: 0.875rem;
		line-height: 1.45;
		overflow-y: auto;
	}
	.chat-input-area textarea::placeholder {
		color: var(--neutral-500);
	}
	.write-target-notice {
		margin: 2px var(--space-2) 4px;
		padding: 7px 9px;
		border: 1px solid color-mix(in srgb, var(--accent-300) 35%, var(--border-default));
		border-radius: var(--radius-sm);
		background: color-mix(in srgb, var(--accent-500) 8%, transparent);
		color: var(--text-secondary);
		font-size: 0.72rem;
		line-height: 1.4;
	}
	.prompt-toolbar {
		display: flex;
		align-items: center;
		gap: 5px;
		padding: 2px;
	}
	.prompt-spacer {
		flex: 1;
	}

	.interaction-mode {
		display: inline-flex;
		height: 28px;
		padding: 2px;
		border: 1px solid var(--border-default);
		border-radius: var(--radius-md);
		background: var(--neutral-950);
	}
	.interaction-mode button {
		padding: 0 9px;
		border: 0;
		border-radius: calc(var(--radius-md) - 2px);
		background: transparent;
		color: var(--text-secondary);
		font-size: 0.68rem;
		font-weight: 600;
		cursor: pointer;
	}
	.interaction-mode button:hover {
		color: var(--text-primary);
	}
	.interaction-mode button.active {
		background: var(--accent-100);
		color: var(--bg-base);
	}

	/* Shared control baseline: same height, calm by default, theme-consistent. */
	.prompt-icon-btn,
	.send-btn,
	.selection-pill {
		height: 28px;
		display: inline-flex;
		align-items: center;
		justify-content: center;
		border: none;
		background: transparent;
		color: var(--text-secondary);
		cursor: pointer;
		border-radius: var(--radius-md);
		white-space: nowrap;
		transition:
			background 0.14s ease,
			color 0.14s ease,
			border-color 0.14s ease,
			box-shadow 0.14s ease,
			opacity 0.14s ease;
	}

	.prompt-icon-btn {
		width: 28px;
	}
	.prompt-icon-btn:hover {
		background: var(--neutral-900);
		color: var(--text-primary);
	}
	.selection-pill {
		gap: 6px;
		padding: 0 5px 0 9px;
		border-radius: var(--radius-md);
		font-size: 0.72rem;
		font-weight: 600;
		color: var(--accent-100);
		background: color-mix(in srgb, var(--accent-100) 9%, transparent);
	}
	.selection-pill:hover {
		background: color-mix(in srgb, var(--accent-100) 15%, transparent);
	}
	.selection-pill svg {
		opacity: 0.75;
	}
	.selection-pill .sel-x {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		width: 16px;
		height: 16px;
		border-radius: 50%;
		opacity: 0.5;
		font-size: 0.66rem;
	}
	.selection-pill:hover .sel-x {
		opacity: 0.95;
		background: color-mix(in srgb, var(--accent-100) 22%, transparent);
	}

	.send-btn {
		width: 28px;
	}
	.send-btn:not(:disabled) {
		background: var(--accent-200);
		color: var(--on-accent);
	}
	.send-btn:hover:not(:disabled) {
		background: var(--accent-100);
	}
	.send-btn:disabled {
		background: transparent;
		color: var(--neutral-600);
		cursor: default;
	}

	.chat-input-area textarea::-webkit-scrollbar {
		width: 6px;
	}
	.chat-input-area textarea::-webkit-scrollbar-track {
		background: transparent;
	}
	.chat-input-area textarea::-webkit-scrollbar-thumb {
		background: var(--neutral-700, #444);
		border-radius: 3px;
	}
	.chat-input-area textarea::-webkit-scrollbar-thumb:hover {
		background: var(--neutral-500, #666);
	}

	.debug-window {
		background: var(--neutral-900, #1a1a2e);
		border: 1px solid var(--border-subtle, #333);
		border-radius: var(--radius-md);
		font-size: 0.7rem;
		font-family: var(--font-mono, monospace);
		color: var(--text-secondary, #aaa);
		overflow: hidden;
	}
	.debug-window-header {
		display: flex;
		justify-content: space-between;
		align-items: center;
		padding: 4px 8px;
		border-bottom: 1px solid var(--border-subtle, #333);
		background: var(--neutral-950, #0d0d1a);
	}
	.debug-title {
		font-weight: 600;
		font-size: 0.68rem;
		text-transform: uppercase;
		letter-spacing: 0.5px;
		color: var(--text-tertiary, #777);
	}
	.debug-toggle {
		background: none;
		border: none;
		color: var(--text-tertiary, #777);
		cursor: pointer;
		padding: 0 2px;
		font-size: 0.85rem;
		line-height: 1;
		border-radius: 3px;
	}
	.debug-toggle:hover {
		color: var(--text-primary);
		background: var(--neutral-800, #222);
	}
	.debug-grid {
		display: grid;
		grid-template-columns: 1fr 1fr;
		gap: 1px;
		padding: 4px 8px;
	}
	.debug-row {
		display: flex;
		justify-content: space-between;
		padding: 1px 0;
	}
	.debug-label {
		color: var(--text-tertiary, #777);
	}
	.debug-value {
		color: var(--text-primary, #ddd);
		font-weight: 500;
		text-align: right;
	}
	.debug-note {
		grid-column: 1 / -1;
		color: var(--text-tertiary, #777);
		font-size: 0.65rem;
		padding: 2px 8px 4px;
		font-style: italic;
	}
	.debug-trace {
		max-height: 120px;
		overflow-y: auto;
		border-top: 1px solid var(--border-subtle, #333);
		padding: 4px 8px;
		display: flex;
		flex-direction: column;
		gap: 2px;
		font-size: 0.65rem;
	}
	.trace-entry {
		display: flex;
		gap: 6px;
		align-items: baseline;
		padding: 1px 0;
	}
	.trace-time {
		color: var(--text-tertiary, #555);
		flex-shrink: 0;
		min-width: 38px;
		font-variant-numeric: tabular-nums;
	}
	.trace-msg {
		color: var(--text-secondary, #aaa);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}
	.trace-prompt {
		flex: 1;
		min-width: 0;
		color: var(--accent-100, #93c5fd);
	}
	.trace-prompt summary {
		cursor: pointer;
		user-select: none;
	}
	.trace-prompt pre {
		margin: 0.4rem 0 0;
		padding: 0.45rem;
		max-height: 18rem;
		overflow: auto;
		white-space: pre-wrap;
		word-break: break-word;
		font: inherit;
		color: var(--text-secondary, #aaa);
		background: var(--bg-raised, rgba(0, 0, 0, 0.2));
		border-radius: 3px;
	}
	.trace-entry.tool .trace-msg {
		color: var(--accent-200, #6ea8fe);
	}
	.trace-entry.gen .trace-msg {
		color: var(--text-primary, #ddd);
	}
	.trace-entry.note .trace-msg {
		color: var(--accent-100, #93c5fd);
	}
	.trace-entry.done .trace-msg {
		color: #4ade80;
	}
	.trace-entry.error .trace-msg {
		color: var(--danger, #ef4444);
	}
	.trace-entry.send .trace-msg {
		color: var(--text-tertiary, #777);
	}
	.trace-entry.config .trace-msg {
		color: #c084fc;
	}
	.trace-entry.usage .trace-msg {
		color: #38bdf8;
	}
	.trace-entry.tool_result .trace-msg {
		color: #4ade80;
	}

	.loading-dots .loading-dots::after {
		content: '...';
		animation: blink 1.5s steps(4, end) infinite;
	}
	@keyframes blink {
		0%,
		20% {
			color: transparent;
		}
		40% {
			color: inherit;
		}
		100% {
			color: inherit;
		}
	}

	.chat-msg-actions {
		display: flex;
		gap: var(--space-1);
		margin-top: 3px;
		justify-content: flex-end;
	}
	.chat-msg-actions.assistant {
		justify-content: flex-start;
	}
	.copy-btn {
		display: inline-flex;
		align-items: center;
		justify-content: center;
	}
	.rewind-btn {
		padding: 1px 6px;
		font-size: 13px;
		background: transparent;
		border: 1px solid transparent;
		border-radius: var(--radius-sm);
		color: color-mix(in srgb, var(--text-tertiary) 50%, transparent);
		cursor: pointer;
		transition:
			color 0.15s,
			border-color 0.15s;
		line-height: 1.6;
	}
	.rewind-btn:hover {
		color: var(--text-secondary);
		border-color: var(--border-color);
	}
	.rewind-btn.retry:hover {
		color: var(--accent);
	}

	/* Version History UI */
	.versions-container {
		display: flex;
		flex-direction: column;
		height: 100%;
	}
	.history-list {
		list-style: none;
		padding: 0;
		margin: 0;
		display: flex;
		flex-direction: column;
		gap: var(--space-4);
	}
	.history-list li {
		background: var(--bg-panel);
		border: 1px solid var(--border-default);
		border-radius: var(--radius-sm);
		padding: var(--space-3);
		display: flex;
		flex-direction: column;
		gap: var(--space-2);
	}
	.commit-header {
		display: flex;
		flex-direction: column;
	}
	.commit-header strong {
		font-size: 0.875rem;
		color: var(--text-primary);
	}
	.commit-date {
		font-size: 0.75rem;
		color: var(--text-secondary);
	}
	.commit-actions {
		display: flex;
		gap: var(--space-2);
		margin-top: var(--space-2);
	}
	.commit-actions button {
		flex: 1;
		font-size: 0.75rem;
		padding: var(--space-1) 0;
	}

	.chat-time-taken {
		display: block;
		font-size: 0.75rem;
		color: var(--text-secondary);
		opacity: 0.7;
		margin-top: var(--space-2);
		user-select: none;
	}

	.chat-time-taken.live {
		color: var(--accent);
		opacity: 0.9;
	}

	.chat-progress {
		display: block;
		color: var(--text-secondary);
		font-size: 0.82rem;
		line-height: 1.35;
		margin-bottom: var(--space-1);
	}

	/* Animated "working" dots shown while a turn is running but has produced no
	   text yet (model thinking, or a tool like web search/fetch executing) — so a
	   slow turn reads as alive, not frozen. */
	.chat-working {
		display: inline-flex;
		gap: 4px;
		align-items: center;
		margin-top: var(--space-2);
		margin-right: var(--space-2);
		vertical-align: middle;
	}

	.chat-working span {
		width: 5px;
		height: 5px;
		border-radius: 50%;
		background: var(--accent);
		animation: chat-working-pulse 1.2s ease-in-out infinite;
	}

	.chat-working span:nth-child(2) {
		animation-delay: 0.2s;
	}

	.chat-working span:nth-child(3) {
		animation-delay: 0.4s;
	}

	@keyframes chat-working-pulse {
		0%,
		80%,
		100% {
			opacity: 0.25;
			transform: scale(0.8);
		}
		40% {
			opacity: 1;
			transform: scale(1);
		}
	}
</style>
