import { invoke } from '@tauri-apps/api/core';

import { open as openFileDialog } from '@tauri-apps/plugin-dialog';

import { goto, beforeNavigate } from '$app/navigation';

import { base, resolve } from '$app/paths';

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
import { get } from 'svelte/store';

import { noteOpened, noteClosed } from '$lib/llamaWarm';

import { chatSidebarShortcut, showSidebarToggle, noteSidebarOpen } from '$lib/stores';

import { shortcutMatches } from '$lib/keyboardShortcut';

import { formatBacklinkContext } from '$lib/backlinkContext';

import { theme } from '$lib/theme';

import type Vditor from 'vditor';

import 'mathlive/fonts.css';

import ChatToolIndicator from '$lib/components/ChatToolIndicator.svelte';

import { hideThinkingContent } from '$lib/chatContent';

import { resolveActiveAiTarget } from '$lib/aiTarget';

import {
		canApplyReconciledNote,
		editorNeedsAuthoritativeBody,
		hasNoteMutation
	} from '$lib/noteMutation';

import { marked } from 'marked';

import DOMPurify from 'dompurify';

import { vditorI18n } from '$lib/vditorI18n';

import { parseBlocks } from './model/links';
import type { BlockItem } from './types';
import { installAiEventBridge } from './sessions/aiEvents.svelte';
import { createLinkingSession } from './sessions/linking.svelte';
import { createSelectionSession } from './sessions/selection.svelte';
import { createLatexSession } from './sessions/latex.svelte';
import { createDocumentSession } from './sessions/document.svelte';
import { createStreamingSession } from './sessions/streaming.svelte';

export function createNotePageController() {
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
		type AiInteractionMode = 'chat' | 'write';
		let aiInteractionMode = $state<AiInteractionMode>('chat');
	
		function setAiInteractionMode(mode: AiInteractionMode) {
			aiInteractionMode = mode;
			if (mode === 'chat') writeTargetNotice = false;
			localStorage.setItem('myelin_ai_interaction_mode', mode);
			if (mode === 'write' && activeSection) {
				const aiNoteId = activeAiNoteId();
				if (aiNoteId) {
					void invoke('warm_llama_server', {
						noteId: aiNoteId,
						interactionMode: mode,
						activeSection
					}).catch((error) => console.debug('Write profile warm-up skipped:', error));
				}
			}
		}
	
		function handleActiveSectionChange(section: ActiveSection) {
			const changed = activeSection?.key !== section.key;
			activeSection = section;
			if (!changed) return;
			const aiNoteId = activeAiNoteId();
			if (aiNoteId) {
				void invoke('warm_llama_server', {
					noteId: aiNoteId,
					interactionMode: aiInteractionMode,
					activeSection: section
				}).catch((error) => console.debug('Section profile warm-up skipped:', error));
			}
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
		let vditorLoading = $state(false);
	
		function localVditorCdn() {
			const appPath = `${base}/vditor/`.replace(/\/+/g, '/');
			return new URL(appPath.startsWith('/') ? appPath : `/${appPath}`, document.baseURI).href.replace(
				/\/$/,
				''
			);
		}
	
		// Keep the note render separate from the editor/tool bundle. The bundle is
		// requested only after the note has had a chance to paint.
		let toolsReady = $state(false);
		let fullscreenShortcut = $state('Esc');
		// Pending tool-approval prompts auto-reject after this long if the user
		// never answers (mirrors the backend's TOOL_APPROVAL_TIMEOUT_SECS).
		const APPROVAL_TIMEOUT_MS = 120_000;
		const approvalTimeouts = new Map<string, ReturnType<typeof setTimeout>>();
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
		type ActiveSection = { key: string; label?: string; content: string };
		let activeSection = $state<ActiveSection | null>(null);
		type SectionCacheState = {
			done: number;
			total: number;
			sectionDone: number;
			sectionTotal: number;
			label: string;
			profile: 'shared' | 'chat' | 'write' | '';
			startedAt: number;
			finished: boolean;
			failed: number;
			failedDetails: string[];
			elapsedMs: number | null;
		};
		let sectionCache = $state<SectionCacheState | null>(null);
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
	
		// The viewer extracted the whole document's sections (PDF pages / EPUB
		// chapters / HTML buckets). Hand them to the backend so every section's KV
		// snapshot is evaluated once, in the background, and saved to disk — by the
		// time the user asks, the active section restores in milliseconds.
		async function handleSectionsReady(sections: ActiveSection[]) {
			if (!sections.length) return;
			const aiNoteId = activeAiNoteId();
			if (!aiNoteId) return;
			// Optimistic start: show the bar immediately; the backend refines it with
			// progress events and always emits a terminal event with which we can stop
			// the elapsed-time clock.
			sectionCache = {
				done: 0,
				total: sections.length,
				sectionDone: 0,
				sectionTotal: sections.length,
				label: sections[0]?.label ?? '',
				profile: '',
				startedAt: performance.now(),
				finished: false,
				failed: 0,
				failedDetails: [],
				elapsedMs: null
			};
			try {
				await invoke('cache_note_sections', {
					noteId: aiNoteId,
					sections,
					activeSectionKey: activeSection?.key ?? sections[0]?.key ?? null,
					interactionMode: null
				});
			} catch (error) {
				if (sectionCache && !sectionCache.finished) {
					sectionCache = {
						...sectionCache,
						failed: Math.max(sectionCache.failed, sectionCache.total - sectionCache.done),
						finished: true,
						elapsedMs: Math.max(0, performance.now() - sectionCache.startedAt)
					};
				}
				console.debug('Section pre-cache skipped:', error);
			}
		}
	
		function formatSectionCacheDuration(milliseconds: number): string {
			if (milliseconds < 1000) return `${Math.round(milliseconds)} ms`;
			const seconds = milliseconds / 1000;
			if (seconds < 60) return `${seconds.toFixed(seconds < 10 ? 1 : 0)} s`;
			const minutes = Math.floor(seconds / 60);
			return `${minutes}m ${Math.round(seconds % 60)}s`;
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
		const SIDEBAR_MIN_WIDTH = 320;
		let sidebarWidth = $state(SIDEBAR_MIN_WIDTH);
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
				const layoutRect = mainLayoutEl?.getBoundingClientRect();
				const containerWidth = layoutRect?.width ?? window.innerWidth;
				// Measure from the actual layout edge rather than the browser window.
				// The note view can be inset by a host shell, and using window.innerWidth
				// made the sidebar grow past the editor when it was resized.
				const newWidth = (layoutRect?.right ?? window.innerWidth) - e.clientX;
				const maxSidebar = Math.max(SIDEBAR_MIN_WIDTH, containerWidth - PANE_MIN_WIDTH);
				sidebarWidth = Math.max(SIDEBAR_MIN_WIDTH, Math.min(newWidth, maxSidebar));
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
				!shortcutMatches(event, get(chatSidebarShortcut)) ||
				!note ||
				event.defaultPrevented
			)
				return;
			event.preventDefault();
			event.stopPropagation();
	
			if (get(noteSidebarOpen) && document.activeElement === chatTextareaEl) {
				noteSidebarOpen.set(false);
				await tick();
				restoreShortcutEditorFocus();
				return;
			}
	
			captureShortcutEditorTarget();
			activeSidebarTab = 'chat';
			noteSidebarOpen.set(true);
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
	
		// A live note stream is starting (whole-body replace). Keep the existing note
		// visible until the first real content arrives; clearing here made fast tool
		// calls flash an empty editor before the authoritative write landed.
		function initVditor() {
			if (!VditorConstructor || !vditorContainer || vditorInstance) return;
	
			try {
				const cdn = localVditorCdn();
				vditorInstance = new VditorConstructor(vditorContainer, {
					value: draftBody,
					cdn,
					_lutePath: `${cdn}/dist/js/lute/lute.min.js`,
					placeholder: isSourceMaterial ? 'Scratchpad for notes...' : 'Start typing here...',
					mode: 'ir',
					// Vditor ships its own skin; 'classic' is its light theme. We mirror the
					// app theme here and keep it in sync via the $effect below. Pass only the
					// skin (no content/code theme) so Vditor doesn't fetch theme CSS from a CDN
					// — the editor's bg/text colors come from our own var overrides anyway.
					theme: get(theme) === 'light' ? 'classic' : 'dark',
					icon: 'material',
					lang: 'en_US',
					i18n: vditorI18n,
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
									linkingSession.linkSearchQuery = '';
									linkingSession.linkSearchResults = [];
									linkingSession.linkNoteDialog?.showModal();
								setTimeout(() => {
									const input = linkingSession.linkNoteDialog?.querySelector(
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
								linkingSession.openGlobalBlockSearch();
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
		const skin = get(theme) === 'light' ? 'classic' : 'dark';
			if (vditorInstance) vditorInstance.setTheme(skin);
		});
	
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
			if (aiInteractionMode === 'write' && !armedEditTarget()) {
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
			if (aiInteractionMode === 'write' && !editorTarget) {
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
			const composerMode = aiInteractionMode === 'write' ? 'editor' : 'chat';
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
						interactionMode: aiInteractionMode,
						activeSection
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
			const timeout = approvalTimeouts.get(id);
			if (timeout) {
				clearTimeout(timeout);
				approvalTimeouts.delete(id);
			}
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
				sectionCache = null;
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
				activeSection = null;
				sectionCache = null;
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
			const targetId = linkingSession.previewNoteTarget?.sourcePdf ?? linkingSession.previewNoteTarget?.id;
			const currentNoteId = note?.id;
			if (!targetId) return null;
			const basePath = `/notes/${encodeURIComponent(targetId)}`;
			if (!currentNoteId) return basePath;
			return `${basePath}?returnTo=/notes/${encodeURIComponent(currentNoteId)}`;
		}
	
		function expandPreviewNoteDirect() {
			const href = buildPreviewExpandHref();
			if (!href) return;
			linkingSession.previewNoteDialog?.close();
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
	
		const latexSession = createLatexSession({
			get note() { return note; },
			get texCompileError() { return texCompileError; },
			set texCompileError(value) { texCompileError = value; },
			get texPreviewStatus() { return texPreviewStatus; },
			set texPreviewStatus(value) { texPreviewStatus = value; },
			get texCacheWarmed() { return texCacheWarmed; },
			set texCacheWarmed(value) { texCacheWarmed = value; },
			get texCompiling() { return texCompiling; },
			set texCompiling(value) { texCompiling = value; },
			get texCompileQueued() { return texCompileQueued; },
			set texCompileQueued(value) { texCompileQueued = value; },
			get isBusy() { return isBusy; },
			set isBusy(value) { isBusy = value; },
			get texRevision() { return texRevision; },
			set texRevision(value) { texRevision = value; },
			get draftBody() { return draftBody; },
			get activeSourceBytes() { return activeSourceBytes; },
			set activeSourceBytes(value) { activeSourceBytes = value; },
			get sourceMaterialType() { return sourceMaterialType; },
			set sourceMaterialType(value) { sourceMaterialType = value; },
			get showAttachedNote() { return showAttachedNote; },
			set showAttachedNote(value) { showAttachedNote = value; },
			get texDiagnostics() { return texDiagnostics; },
			set texDiagnostics(value) { texDiagnostics = value; },
			get latexDownloadMsg() { return latexDownloadMsg; },
			set latexDownloadMsg(value) { latexDownloadMsg = value; },
			get activeSection() { return activeSection; },
			set activeSection(value) { activeSection = value; },
			get sectionCache() { return sectionCache; },
			set sectionCache(value) { sectionCache = value; },
			get workingDocType() { return workingDocType; },
			get lastTexBody() { return lastTexBody; },
			set lastTexBody(value) { lastTexBody = value; },
			get texAutoCompile() { return texAutoCompile; },
			get texAutoTimer() { return texAutoTimer; },
			set texAutoTimer(value) { texAutoTimer = value; },
			saveNote
		});
		const pickLatexImage = latexSession.pickLatexImage;
		const compileTex = latexSession.compileTex;
		const parseLatexError = latexSession.parseLatexError;
		const closeTexPreview = latexSession.closeTexPreview;

		const selectionSession = createSelectionSession({
			get vditorInstance() { return vditorInstance; },
			get vditorContainer() { return vditorContainer; },
			get armedSelection() { return armedSelection; },
			set armedSelection(value) { armedSelection = value; },
			get writeTargetNotice() { return writeTargetNotice; },
			set writeTargetNotice(value) { writeTargetNotice = value; },
			get selDebounce() { return selDebounce; },
			set selDebounce(value) { selDebounce = value; },
			get savedEditorRange() { return savedEditorRange; },
			set savedEditorRange(value) { savedEditorRange = value; },
			get draftBody() { return draftBody; },
			set draftBody(value) { draftBody = value; },
			triggerAutoSave,
			focusEditor
		});
		const getSelectionTextOffset = selectionSession.getSelectionTextOffset;
		const textOffsetOf = selectionSession.textOffsetOf;
		const nearestIndexOf = selectionSession.nearestIndexOf;
		const computeSourceSelection = selectionSession.computeSourceSelection;
		const computeSourceCursor = selectionSession.computeSourceCursor;
		const clearArmedSelection = selectionSession.clearArmedSelection;
		const onDocMouseDown = selectionSession.onDocMouseDown;
		const captureEditorSelection = selectionSession.captureEditorSelection;
		const captureExternalTarget = selectionSession.captureExternalTarget;
		const reselectAfterEdit = selectionSession.reselectAfterEdit;
		const armedEditTarget = selectionSession.armedEditTarget;
		const onSelectionChange = selectionSession.onSelectionChange;
		const restoreSelectionTextOffset = selectionSession.restoreSelectionTextOffset;
		const saveCursorPosition = selectionSession.saveCursorPosition;
		const insertAtSavedCursor = selectionSession.insertAtSavedCursor;

		const documentSession = createDocumentSession({
			get isLoadingNote() { return isLoadingNote; },
			set isLoadingNote(value) { isLoadingNote = value; },
			get toolsReady() { return toolsReady; },
			set toolsReady(value) { toolsReady = value; },
			clearArmedSelection,
			get writeTargetNotice() { return writeTargetNotice; },
			set writeTargetNotice(value) { writeTargetNotice = value; },
			get chatPersistTimer() { return chatPersistTimer; },
			set chatPersistTimer(value) { chatPersistTimer = value; },
			activeAiNoteId,
			persistChatHistory,
			destroyEditorInstance,
			get activeSourceBytes() { return activeSourceBytes; },
			set activeSourceBytes(value) { activeSourceBytes = value; },
			get activeSourceId() { return activeSourceId; },
			set activeSourceId(value) { activeSourceId = value; },
			get activeSection() { return activeSection; },
			set activeSection(value) { activeSection = value; },
			get sectionCache() { return sectionCache; },
			set sectionCache(value) { sectionCache = value; },
			get showAttachedNote() { return showAttachedNote; },
			set showAttachedNote(value) { showAttachedNote = value; },
			get note() { return note; },
			set note(value) { note = value; },
			get chatMessages() { return chatMessages; },
			set chatMessages(value) { chatMessages = value; },
			get noteHistory() { return noteHistory; },
			set noteHistory(value) { noteHistory = value; },
			get versionPreviewContent() { return versionPreviewContent; },
			set versionPreviewContent(value) { versionPreviewContent = value; },
			get activeSidebarTab() { return activeSidebarTab; },
			set activeSidebarTab(value) { activeSidebarTab = value; },
			get isSourceMaterial() { return isSourceMaterial; },
			set isSourceMaterial(value) { isSourceMaterial = value; },
			get sourceMaterialType() { return sourceMaterialType; },
			set sourceMaterialType(value) { sourceMaterialType = value; },
			get workingDocType() { return workingDocType; },
			set workingDocType(value) { workingDocType = value; },
			get draftTitle() { return draftTitle; },
			set draftTitle(value) { draftTitle = value; },
			get draftBody() { return draftBody; },
			set draftBody(value) { draftBody = value; },
			get draftTags() { return draftTags; },
			set draftTags(value) { draftTags = value; },
			get scratchpadSavedId() { return scratchpadSavedId; },
			set scratchpadSavedId(value) { scratchpadSavedId = value; },
			get message() { return message; },
			set message(value) { message = value; },
			fetchRelatedNotes,
			get vditorInstance() { return vditorInstance; }
		});
		const loadCurrentNote = documentSession.loadCurrentNote;
		const refreshCurrentNoteFromBackend = documentSession.refreshCurrentNoteFromBackend;
		const streamingSession = createStreamingSession({
			get noteStreamBackup() { return noteStreamBackup; },
			set noteStreamBackup(value) { noteStreamBackup = value; },
			get noteStreamBuf() { return noteStreamBuf; },
			set noteStreamBuf(value) { noteStreamBuf = value; },
			get noteStreaming() { return noteStreaming; },
			set noteStreaming(value) { noteStreaming = value; },
			get transclusionObserver() { return transclusionObserver; },
			get activeAiEditTarget() { return activeAiEditTarget; },
			get noteStreamSpan() { return noteStreamSpan; },
			set noteStreamSpan(value) { noteStreamSpan = value; },
			get noteStreamFlushPending() { return noteStreamFlushPending; },
			set noteStreamFlushPending(value) { noteStreamFlushPending = value; },
			get draftBody() { return draftBody; },
			set draftBody(value) { draftBody = value; },
			get vditorInstance() { return vditorInstance; },
			get vditorContainer() { return vditorContainer; },
			get note() { return note; },
			set note(value) { note = value; },
			getSelectionTextOffset,
			restoreSelectionTextOffset,
			setupTransclusionObserver
		});
		const beginNoteStream = streamingSession.beginNoteStream;
		const scheduleNoteStreamFlush = streamingSession.scheduleNoteStreamFlush;
		const flushNoteStream = streamingSession.flushNoteStream;
		const appendNoteStream = streamingSession.appendNoteStream;
		const cancelNoteStream = streamingSession.cancelNoteStream;
		const applyNoteWrite = streamingSession.applyNoteWrite;

		const linkingSession = createLinkingSession({
			get isBusy() { return isBusy; },
			set isBusy(value) { isBusy = value; },
			get VditorConstructor() { return VditorConstructor; },
			localVditorCdn,
			get shouldRefocusEditor() { return shouldRefocusEditor; },
			set shouldRefocusEditor(value) { shouldRefocusEditor = value; },
			insertAtSavedCursor,
			refocusEditorSoon,
			get vditorInstance() { return vditorInstance; },
			get vditorContainer() { return vditorContainer; },
			getSelectionTextOffset,
			get draftBody() { return draftBody; },
			set draftBody(value) { draftBody = value; },
			get note() { return note; },
			get message() { return message; },
			set message(value) { message = value; },
			saveCursorPosition,
			focusEditor,
			restoreSelectionTextOffset
		});

		const aiEventContext: Record<string, any> = {
			get note() { return note; },
			set note(value) { note = value; },
			get message() { return message; },
			set message(value) { message = value; },
			get latexDownloadMsg() { return latexDownloadMsg; },
			set latexDownloadMsg(value) { latexDownloadMsg = value; },
			get texCacheWarmed() { return texCacheWarmed; },
			set texCacheWarmed(value) { texCacheWarmed = value; },
			get chatMessages() { return chatMessages; },
			set chatMessages(value) { chatMessages = value; },
			get chatChunkBuf() { return chatChunkBuf; },
			set chatChunkBuf(value) { chatChunkBuf = value; },
			get chatChunkFlushPending() { return chatChunkFlushPending; },
			set chatChunkFlushPending(value) { chatChunkFlushPending = value; },
			get showDebugWindow() { return showDebugWindow; },
			get pendingDebugTrace() { return pendingDebugTrace; },
			set pendingDebugTrace(value) { pendingDebugTrace = value; },
			get activeAiComposerMode() { return activeAiComposerMode; },
			get debugInfo() { return debugInfo; },
			set debugInfo(value) { debugInfo = value; },
			get armedSelection() { return armedSelection; },
			get activeChatRequestId() { return activeChatRequestId; },
			get chatMessagesEl() { return chatMessagesEl; },
			get workingDocType() { return workingDocType; },
			get sectionCache() { return sectionCache; },
			set sectionCache(value) { sectionCache = value; },
			APPROVAL_TIMEOUT_MS,
			approvalTimeouts,
			MAX_DEBUG_TRACE,
			activeAiNoteId,
			flushChatChunks,
			makeDebugTraceEntry,
			setStreamingStatus,
			visibleAiStatus,
			scrollChatToBottom,
			clearArmedSelection,
			reselectAfterEdit,
			beginNoteStream,
			appendNoteStream,
			cancelNoteStream,
			applyNoteWrite,
			finishStreamingChatMessage,
			failStreamingChatMessage,
			resolveApproval
		};

		onMount(() => {
			// Warm llama-server (safety net — the server is already started at app
			// boot and stays warm for the entire session).
			const savedInteractionMode = localStorage.getItem('myelin_ai_interaction_mode');
			aiInteractionMode = savedInteractionMode === 'operation' || savedInteractionMode === 'write' ? 'write' : 'chat';
			const savedSidebarWidth = localStorage.getItem('myelin_sidebar_width');
			if (savedSidebarWidth) {
				const parsed = parseInt(savedSidebarWidth, 10);
				if (!isNaN(parsed)) {
					const maxSidebar = Math.max(
						SIDEBAR_MIN_WIDTH,
						window.innerWidth - PANE_MIN_WIDTH
					);
					sidebarWidth = Math.max(SIDEBAR_MIN_WIDTH, Math.min(parsed, maxSidebar));
				}
			}
	
		showSidebarToggle.set(true);
			// The note sidebar's open/closed state is remembered across sessions via the
			// persisted noteSidebarOpen store, so we intentionally don't force it here.
	
			const mql = window.matchMedia('(max-width: 1200px)');
			const handleMediaChange = (_e: MediaQueryListEvent) => {};
			mql.addEventListener('change', handleMediaChange);
			document.addEventListener('selectionchange', handleGlobalSelectionChange);
			document.addEventListener('mousedown', onDocMouseDown, true);
			window.addEventListener('keydown', handleChatSidebarShortcut, true);
			const disposeAiEvents = installAiEventBridge(aiEventContext);
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
			showSidebarToggle.set(false);
	
				disposeAiEvents();
			};
		});
	
		onDestroy(() => {
			// Note view closing — server stays warm (started at app boot, lives until
			// app exit). Only the note-editor UI is torn down.
			if (chatPersistTimer) clearTimeout(chatPersistTimer);
			if (texAutoTimer) clearTimeout(texAutoTimer);
			for (const timeout of approvalTimeouts.values()) clearTimeout(timeout);
			approvalTimeouts.clear();
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

	return {
		get requireToolApproval() { return requireToolApproval; },
		set requireToolApproval(value: typeof requireToolApproval) { requireToolApproval = value; },
		get note() { return note; },
		set note(value: typeof note) { note = value; },
		get isLoadingNote() { return isLoadingNote; },
		set isLoadingNote(value: typeof isLoadingNote) { isLoadingNote = value; },
		get draftBody() { return draftBody; },
		set draftBody(value: typeof draftBody) { draftBody = value; },
		get draftTitle() { return draftTitle; },
		set draftTitle(value: typeof draftTitle) { draftTitle = value; },
		get draftTags() { return draftTags; },
		set draftTags(value: typeof draftTags) { draftTags = value; },
		get isBusy() { return isBusy; },
		set isBusy(value: typeof isBusy) { isBusy = value; },
		get message() { return message; },
		set message(value: typeof message) { message = value; },
		get latexDownloadMsg() { return latexDownloadMsg; },
		set latexDownloadMsg(value: typeof latexDownloadMsg) { latexDownloadMsg = value; },
		get texAutoCompile() { return texAutoCompile; },
		set texAutoCompile(value: typeof texAutoCompile) { texAutoCompile = value; },
		get texCompiling() { return texCompiling; },
		set texCompiling(value: typeof texCompiling) { texCompiling = value; },
		get texCacheWarmed() { return texCacheWarmed; },
		set texCacheWarmed(value: typeof texCacheWarmed) { texCacheWarmed = value; },
		get texPreviewStatus() { return texPreviewStatus; },
		set texPreviewStatus(value: typeof texPreviewStatus) { texPreviewStatus = value; },
		get texCompileError() { return texCompileError; },
		set texCompileError(value: typeof texCompileError) { texCompileError = value; },
		get texRevision() { return texRevision; },
		set texRevision(value: typeof texRevision) { texRevision = value; },
		get texCompileQueued() { return texCompileQueued; },
		set texCompileQueued(value: typeof texCompileQueued) { texCompileQueued = value; },
		get lastTexBody() { return lastTexBody; },
		set lastTexBody(value: typeof lastTexBody) { lastTexBody = value; },
		get texDiagnostics() { return texDiagnostics; },
		set texDiagnostics(value: typeof texDiagnostics) { texDiagnostics = value; },
		get texAutoTimer() { return texAutoTimer; },
		set texAutoTimer(value: typeof texAutoTimer) { texAutoTimer = value; },
		get activeSidebarTab() { return activeSidebarTab; },
		set activeSidebarTab(value: typeof activeSidebarTab) { activeSidebarTab = value; },
		get noteHistory() { return noteHistory; },
		set noteHistory(value: typeof noteHistory) { noteHistory = value; },
		get versionPreviewContent() { return versionPreviewContent; },
		set versionPreviewContent(value: typeof versionPreviewContent) { versionPreviewContent = value; },
		get versionPreviewHash() { return versionPreviewHash; },
		set versionPreviewHash(value: typeof versionPreviewHash) { versionPreviewHash = value; },
		get versionPreviewDialog() { return versionPreviewDialog; },
		set versionPreviewDialog(value: typeof versionPreviewDialog) { versionPreviewDialog = value; },
		get chatMessages() { return chatMessages; },
		set chatMessages(value: typeof chatMessages) { chatMessages = value; },
		get chatInput() { return chatInput; },
		set chatInput(value: typeof chatInput) { chatInput = value; },
		get copiedIdx() { return copiedIdx; },
		set copiedIdx(value: typeof copiedIdx) { copiedIdx = value; },
		get chatChunkBuf() { return chatChunkBuf; },
		set chatChunkBuf(value: typeof chatChunkBuf) { chatChunkBuf = value; },
		get chatChunkFlushPending() { return chatChunkFlushPending; },
		set chatChunkFlushPending(value: typeof chatChunkFlushPending) { chatChunkFlushPending = value; },
		get chatPersistTimer() { return chatPersistTimer; },
		set chatPersistTimer(value: typeof chatPersistTimer) { chatPersistTimer = value; },
		persistableChatHistory,
		persistChatHistory,
		checkpointChatHistory,
		flushChatChunks,
		get showDebugWindow() { return showDebugWindow; },
		set showDebugWindow(value: typeof showDebugWindow) { showDebugWindow = value; },
		get MAX_DEBUG_TRACE() { return MAX_DEBUG_TRACE; },
		get MAX_DEBUG_MSG_CHARS() { return MAX_DEBUG_MSG_CHARS; },
		makeDebugTraceEntry,
		get chatRenderCache() { return chatRenderCache; },
		get MAX_CHAT_RENDER_CACHE() { return MAX_CHAT_RENDER_CACHE; },
		renderChatContent,
		get pendingDebugTrace() { return pendingDebugTrace; },
		set pendingDebugTrace(value: typeof pendingDebugTrace) { pendingDebugTrace = value; },
		get activeAiComposerMode() { return activeAiComposerMode; },
		set activeAiComposerMode(value: typeof activeAiComposerMode) { activeAiComposerMode = value; },
		get activeChatNoteId() { return activeChatNoteId; },
		set activeChatNoteId(value: typeof activeChatNoteId) { activeChatNoteId = value; },
		get aiInteractionMode() { return aiInteractionMode; },
		set aiInteractionMode(value: typeof aiInteractionMode) { aiInteractionMode = value; },
		setAiInteractionMode,
		handleActiveSectionChange,
		setToolApproval,
		setStreamingStatus,
		visibleAiStatus,
		get debugInfo() { return debugInfo; },
		set debugInfo(value: typeof debugInfo) { debugInfo = value; },
		copyMessage,
		get armedSelection() { return armedSelection; },
		set armedSelection(value: typeof armedSelection) { armedSelection = value; },
		get selDebounce() { return selDebounce; },
		set selDebounce(value: typeof selDebounce) { selDebounce = value; },
		get activeAiEditTarget() { return activeAiEditTarget; },
		set activeAiEditTarget(value: typeof activeAiEditTarget) { activeAiEditTarget = value; },
		get writeTargetNotice() { return writeTargetNotice; },
		set writeTargetNotice(value: typeof writeTargetNotice) { writeTargetNotice = value; },
		get activeChatRequestId() { return activeChatRequestId; },
		set activeChatRequestId(value: typeof activeChatRequestId) { activeChatRequestId = value; },
		get isChatStreaming() { return isChatStreaming; },
		set isChatStreaming(value: typeof isChatStreaming) { isChatStreaming = value; },
		openNoteNotebook,
		attachFile,
		get chatTextareaEl() { return chatTextareaEl; },
		set chatTextareaEl(value: typeof chatTextareaEl) { chatTextareaEl = value; },
		get chatMessagesEl() { return chatMessagesEl; },
		set chatMessagesEl(value: typeof chatMessagesEl) { chatMessagesEl = value; },
		get currentTime() { return currentTime; },
		set currentTime(value: typeof currentTime) { currentTime = value; },
		get debugTimer() { return debugTimer; },
		set debugTimer(value: typeof debugTimer) { debugTimer = value; },
		get backUrl() { return backUrl; },
		set backUrl(value: typeof backUrl) { backUrl = value; },
		get relatedNotes() { return relatedNotes; },
		set relatedNotes(value: typeof relatedNotes) { relatedNotes = value; },
		get vditorContainer() { return vditorContainer; },
		set vditorContainer(value: typeof vditorContainer) { vditorContainer = value; },
		get vditorInstance() { return vditorInstance; },
		set vditorInstance(value: typeof vditorInstance) { vditorInstance = value; },
		get VditorConstructor() { return VditorConstructor; },
		set VditorConstructor(value: typeof VditorConstructor) { VditorConstructor = value; },
		get vditorLoading() { return vditorLoading; },
		set vditorLoading(value: typeof vditorLoading) { vditorLoading = value; },
		localVditorCdn,
		get toolsReady() { return toolsReady; },
		set toolsReady(value: typeof toolsReady) { toolsReady = value; },
		get fullscreenShortcut() { return fullscreenShortcut; },
		set fullscreenShortcut(value: typeof fullscreenShortcut) { fullscreenShortcut = value; },
		get APPROVAL_TIMEOUT_MS() { return APPROVAL_TIMEOUT_MS; },
		get approvalTimeouts() { return approvalTimeouts; },
		get noteStreaming() { return noteStreaming; },
		set noteStreaming(value: typeof noteStreaming) { noteStreaming = value; },
		get noteStreamBuf() { return noteStreamBuf; },
		set noteStreamBuf(value: typeof noteStreamBuf) { noteStreamBuf = value; },
		get noteStreamBackup() { return noteStreamBackup; },
		set noteStreamBackup(value: typeof noteStreamBackup) { noteStreamBackup = value; },
		get noteStreamFlushPending() { return noteStreamFlushPending; },
		set noteStreamFlushPending(value: typeof noteStreamFlushPending) { noteStreamFlushPending = value; },
		get noteStreamSpan() { return noteStreamSpan; },
		set noteStreamSpan(value: typeof noteStreamSpan) { noteStreamSpan = value; },
		get savedEditorRange() { return savedEditorRange; },
		set savedEditorRange(value: typeof savedEditorRange) { savedEditorRange = value; },
		get shortcutEditorRange() { return shortcutEditorRange; },
		set shortcutEditorRange(value: typeof shortcutEditorRange) { shortcutEditorRange = value; },
		get shouldRefocusEditor() { return shouldRefocusEditor; },
		set shouldRefocusEditor(value: typeof shouldRefocusEditor) { shouldRefocusEditor = value; },
		get isSourceMaterial() { return isSourceMaterial; },
		set isSourceMaterial(value: typeof isSourceMaterial) { isSourceMaterial = value; },
		get sourceMaterialType() { return sourceMaterialType; },
		set sourceMaterialType(value: typeof sourceMaterialType) { sourceMaterialType = value; },
		get workingDocType() { return workingDocType; },
		set workingDocType(value: typeof workingDocType) { workingDocType = value; },
		get PdfViewerComponent() { return PdfViewerComponent; },
		set PdfViewerComponent(value: typeof PdfViewerComponent) { PdfViewerComponent = value; },
		get EpubViewerComponent() { return EpubViewerComponent; },
		set EpubViewerComponent(value: typeof EpubViewerComponent) { EpubViewerComponent = value; },
		get HtmlViewerComponent() { return HtmlViewerComponent; },
		set HtmlViewerComponent(value: typeof HtmlViewerComponent) { HtmlViewerComponent = value; },
		get TexEditorComponent() { return TexEditorComponent; },
		set TexEditorComponent(value: typeof TexEditorComponent) { TexEditorComponent = value; },
		get IpynbEditorComponent() { return IpynbEditorComponent; },
		set IpynbEditorComponent(value: typeof IpynbEditorComponent) { IpynbEditorComponent = value; },
		get texEditorInstance() { return texEditorInstance; },
		set texEditorInstance(value: typeof texEditorInstance) { texEditorInstance = value; },
		get ipynbEditorInstance() { return ipynbEditorInstance; },
		set ipynbEditorInstance(value: typeof ipynbEditorInstance) { ipynbEditorInstance = value; },
		get activeSourceId() { return activeSourceId; },
		set activeSourceId(value: typeof activeSourceId) { activeSourceId = value; },
		get activeSourceBytes() { return activeSourceBytes; },
		set activeSourceBytes(value: typeof activeSourceBytes) { activeSourceBytes = value; },
		get activeSection() { return activeSection; },
		set activeSection(value: typeof activeSection) { activeSection = value; },
		get sectionCache() { return sectionCache; },
		set sectionCache(value: typeof sectionCache) { sectionCache = value; },
		get scratchpadSavedId() { return scratchpadSavedId; },
		set scratchpadSavedId(value: typeof scratchpadSavedId) { scratchpadSavedId = value; },
		get showAttachedNote() { return showAttachedNote; },
		set showAttachedNote(value: typeof showAttachedNote) { showAttachedNote = value; },
		get pdfIngestionStatus() { return pdfIngestionStatus; },
		set pdfIngestionStatus(value: typeof pdfIngestionStatus) { pdfIngestionStatus = value; },
		get pdfIngestionError() { return pdfIngestionError; },
		set pdfIngestionError(value: typeof pdfIngestionError) { pdfIngestionError = value; },
		get pdfIngestionPromise() { return pdfIngestionPromise; },
		set pdfIngestionPromise(value: typeof pdfIngestionPromise) { pdfIngestionPromise = value; },
		get splitRatio() { return splitRatio; },
		set splitRatio(value: typeof splitRatio) { splitRatio = value; },
		get isResizing() { return isResizing; },
		set isResizing(value: typeof isResizing) { isResizing = value; },
		get mainLayoutEl() { return mainLayoutEl; },
		set mainLayoutEl(value: typeof mainLayoutEl) { mainLayoutEl = value; },
		activeAiNoteId,
		handleSectionsReady,
		formatSectionCacheDuration,
		openAttachedNote,
		get PANE_MIN_WIDTH() { return PANE_MIN_WIDTH; },
		get SIDEBAR_MIN_WIDTH() { return SIDEBAR_MIN_WIDTH; },
		get sidebarWidth() { return sidebarWidth; },
		set sidebarWidth(value: typeof sidebarWidth) { sidebarWidth = value; },
		get isSidebarResizing() { return isSidebarResizing; },
		set isSidebarResizing(value: typeof isSidebarResizing) { isSidebarResizing = value; },
		startSidebarResizing,
		startResizing,
		handleGlobalMouseMove,
		stopResizing,
		handlePdfQuote,
		focusEditor,
		refocusEditorSoon,
		captureShortcutEditorTarget,
		restoreShortcutEditorFocus,
		handleChatSidebarShortcut,
		get userScrolledUp() { return userScrolledUp; },
		set userScrolledUp(value: typeof userScrolledUp) { userScrolledUp = value; },
		handleChatScroll,
		scrollChatToBottom,
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
		insertAtSavedCursor,
		get mathDialog() { return mathDialog; },
		set mathDialog(value: typeof mathDialog) { mathDialog = value; },
		get mathValue() { return mathValue; },
		set mathValue(value: typeof mathValue) { mathValue = value; },
		get mathLiveReady() { return mathLiveReady; },
		set mathLiveReady(value: typeof mathLiveReady) { mathLiveReady = value; },
		get katexRenderer() { return katexRenderer; },
		set katexRenderer(value: typeof katexRenderer) { katexRenderer = value; },
		get mathError() { return mathError; },
		set mathError(value: typeof mathError) { mathError = value; },
		mathToKatex,
		openMathDialog,
		get blockCache() { return blockCache; },
		set blockCache(value: typeof blockCache) { blockCache = value; },
		get transclusionObserver() { return transclusionObserver; },
		set transclusionObserver(value: typeof transclusionObserver) { transclusionObserver = value; },
		get toolbarExpanded() { return toolbarExpanded; },
		set toolbarExpanded(value: typeof toolbarExpanded) { toolbarExpanded = value; },
		get toolbarNeedsToggle() { return toolbarNeedsToggle; },
		set toolbarNeedsToggle(value: typeof toolbarNeedsToggle) { toolbarNeedsToggle = value; },
		get toolbarResizeObserver() { return toolbarResizeObserver; },
		set toolbarResizeObserver(value: typeof toolbarResizeObserver) { toolbarResizeObserver = value; },
		get saveStatus() { return saveStatus; },
		set saveStatus(value: typeof saveStatus) { saveStatus = value; },
		get saveTimer() { return saveTimer; },
		set saveTimer(value: typeof saveTimer) { saveTimer = value; },
		get navigationWarningDialog() { return navigationWarningDialog; },
		set navigationWarningDialog(value: typeof navigationWarningDialog) { navigationWarningDialog = value; },
		get deleteAttachedNoteDialog() { return deleteAttachedNoteDialog; },
		set deleteAttachedNoteDialog(value: typeof deleteAttachedNoteDialog) { deleteAttachedNoteDialog = value; },
		get deleteMainNoteDialog() { return deleteMainNoteDialog; },
		set deleteMainNoteDialog(value: typeof deleteMainNoteDialog) { deleteMainNoteDialog = value; },
		get detachPdfDialog() { return detachPdfDialog; },
		set detachPdfDialog(value: typeof detachPdfDialog) { detachPdfDialog = value; },
		requestDeleteMainNote,
		get pendingNavigationUrl() { return pendingNavigationUrl; },
		set pendingNavigationUrl(value: typeof pendingNavigationUrl) { pendingNavigationUrl = value; },
		get pendingBack() { return pendingBack; },
		set pendingBack(value: typeof pendingBack) { pendingBack = value; },
		get attachPdfDialog() { return attachPdfDialog; },
		set attachPdfDialog(value: typeof attachPdfDialog) { attachPdfDialog = value; },
		get pdfSearchQuery() { return pdfSearchQuery; },
		set pdfSearchQuery(value: typeof pdfSearchQuery) { pdfSearchQuery = value; },
		get pdfNotesList() { return pdfNotesList; },
		set pdfNotesList(value: typeof pdfNotesList) { pdfNotesList = value; },
		get pdfSelectedIndex() { return pdfSelectedIndex; },
		set pdfSelectedIndex(value: typeof pdfSelectedIndex) { pdfSelectedIndex = value; },
		get filteredPdfs() { return filteredPdfs; },
		set filteredPdfs(value: typeof filteredPdfs) { filteredPdfs = value; },
		get shouldRenderEditor() { return shouldRenderEditor; },
		set shouldRenderEditor(value: typeof shouldRenderEditor) { shouldRenderEditor = value; },
		get shouldInitEditor() { return shouldInitEditor; },
		set shouldInitEditor(value: typeof shouldInitEditor) { shouldInitEditor = value; },
		get loadedRouteNoteId() { return loadedRouteNoteId; },
		set loadedRouteNoteId(value: typeof loadedRouteNoteId) { loadedRouteNoteId = value; },
		appendToNoteBody,
		destroyEditorInstance,
		triggerAutoSave,
		insertMath,
		pickLatexImage,
		compileTex,
		parseLatexError,
		closeTexPreview,
		parseBlocks,
		...linkingSession,
		loadCurrentNote,
		refreshCurrentNoteFromBackend,
		beginNoteStream,
		scheduleNoteStreamFlush,
		flushNoteStream,
		appendNoteStream,
		cancelNoteStream,
		applyNoteWrite,
		initVditor,
		scanForTransclusions,
		setupTransclusionObserver,
		fetchRelatedNotes,
		handleAnnotationsChange,
		handleImageExtract,
		handlePdfTextExtracted,
		saveNote,
		deleteNote,
		duplicateNote,
		stopActiveChat,
		stopChat,
		beginAiRequest,
		sendChatMessage,
		sendChatText,
		rewindToSnapshot,
		retryMessage,
		mergeChatTools,
		reconcileRequestNote,
		finishStreamingChatMessage,
		extractChatErrorMessage,
		failStreamingChatMessage,
		resolveApproval,
		fetchNoteHistory,
		previewVersion,
		restoreVersion,
		get isProgrammaticNavigation() { return isProgrammaticNavigation; },
		set isProgrammaticNavigation(value: typeof isProgrammaticNavigation) { isProgrammaticNavigation = value; },
		safeNavigate,
		goBack,
		navigateBack,
		requestDeleteAttachedNote,
		confirmDeleteAttachedNote,
		cancelDeleteAttachedNote,
		openAttachPdfDialog,
		attachPdf,
		requestDetachPdf,
		confirmDetachPdf,
		browseAndAttachPdf,
		handlePdfSearchKeydown,
		buildPreviewExpandHref,
		expandPreviewNoteDirect,
		handleBeforeUnload,
		confirmNavigation,
		cancelNavigation,
		updateToolbarOverflow,
		handleGlobalSelectionChange,
		get debugTraceEl() { return debugTraceEl; },
		set debugTraceEl(value: typeof debugTraceEl) { debugTraceEl = value; },
	};
}

export type NotePageController = ReturnType<typeof createNotePageController>;
