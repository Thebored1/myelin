import { invoke } from '@tauri-apps/api/core';

import { goto } from '$app/navigation';

import { base, resolve } from '$app/paths';

import { page } from '$app/state';

import type {
		NoteDocument,
		NoteSummary,
		PdfAnnotation,
		GitCommit,
		ChatMessage
	} from '$lib/types';

import { tick } from 'svelte';
import { get } from 'svelte/store';

import { noteOpened } from '$lib/llamaWarm';

import { chatSidebarShortcut, noteSidebarOpen } from '$lib/stores';

import { shortcutMatches } from '$lib/keyboardShortcut';

import { formatBacklinkContext } from '$lib/backlinkContext';

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

import type { BlockItem } from './types';
import { createNotePageLifecycle } from './sessions/lifecycle.svelte';
import { createLinkingSession } from './sessions/linking.svelte';
import { createNoteInputGraph } from './sessions/input-graph.svelte';
import { createDocumentSession } from './sessions/document.svelte';
import { createStreamingSession } from './sessions/streaming.svelte';
import { createEditorSession } from './sessions/editor.svelte';
import { createEditorInteractionSession } from './sessions/editor-interaction.svelte';
import { createEditorStateSession } from './sessions/editor-state.svelte';
import { createChatSession } from './sessions/chat.svelte';
import { createNavigationSession } from './sessions/navigation.svelte';
import { createSourceSession } from './sessions/source.svelte';
import { createNotePageGraph } from './sessions/graph.svelte';

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
		let pendingDebugTrace = $state<DebugTraceEntry[]>([]);
		let activeAiComposerMode: 'chat' | 'editor' | null = null;
		let activeChatNoteId: string | null = null;
		type AiInteractionMode = 'chat' | 'write';
		let aiInteractionMode = $state<AiInteractionMode>('chat');
	
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
	
		const PANE_MIN_WIDTH = 26 * 16;
		const SIDEBAR_MIN_WIDTH = 320;
		let sidebarWidth = $state(SIDEBAR_MIN_WIDTH);
		let isSidebarResizing = $state(false);
	
		let selectionSession: Record<string, any>;
		const editorInteraction = createEditorInteractionSession({
			get vditorInstance() { return vditorInstance; },
			get vditorContainer() { return vditorContainer; },
			get workingDocType() { return workingDocType; },
			get texEditorInstance() { return texEditorInstance; },
			get ipynbEditorInstance() { return ipynbEditorInstance; },
			get shortcutEditorRange() { return shortcutEditorRange; },
			set shortcutEditorRange(value) { shortcutEditorRange = value; },
			get shouldRefocusEditor() { return shouldRefocusEditor; },
			set shouldRefocusEditor(value) { shouldRefocusEditor = value; },
			captureEditorSelection: () => selectionSession.captureEditorSelection()
		});
		const { focusEditor, refocusEditorSoon, captureShortcutEditorTarget, restoreShortcutEditorFocus } = editorInteraction;
	
		let userScrolledUp = false;
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
		let saveNote: () => Promise<void> = async () => {};
		const editorState = createEditorStateSession({
			get showAttachedNote() { return showAttachedNote; },
			set showAttachedNote(value) { showAttachedNote = value; },
			get vditorInstance() { return vditorInstance; },
			set vditorInstance(value) { vditorInstance = value; },
			get draftBody() { return draftBody; },
			set draftBody(value) { draftBody = value; },
			get saveStatus() { return saveStatus; },
			set saveStatus(value) { saveStatus = value; },
			get saveTimer() { return saveTimer; },
			set saveTimer(value) { saveTimer = value; },
			saveNote: () => saveNote()
		});
		const { appendToNoteBody, destroyEditorInstance, triggerAutoSave } = editorState;
	
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
	
	
	
		function requestDeleteAttachedNote() {
			deleteAttachedNoteDialog?.showModal();
		}

		async function confirmDeleteAttachedNote() {
			deleteAttachedNoteDialog?.close();
			const targetId = isSourceMaterial ? scratchpadSavedId : note?.sourcePdf ? note.id : null;
			const sourceId = isSourceMaterial ? activeSourceId : (note?.sourcePdf ?? activeSourceId);
			isBusy = true;
			try {
				if (targetId) await invoke('delete_note', { noteId: targetId });
				if (!isSourceMaterial && sourceId) {
					navigationSession.isProgrammaticNavigation = true;
					await goto(`/notes/${encodeURIComponent(sourceId)}`);
					return;
				}
				if (saveTimer) { clearTimeout(saveTimer); saveTimer = null; }
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
			navigationSession.isProgrammaticNavigation = true;
			window.location.href = href;
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
	
		const inputGraph = createNoteInputGraph({
			get note() { return note; },
			get texCompileError() { return texCompileError; }, set texCompileError(value) { texCompileError = value; },
			get texPreviewStatus() { return texPreviewStatus; }, set texPreviewStatus(value) { texPreviewStatus = value; },
			get texCacheWarmed() { return texCacheWarmed; }, set texCacheWarmed(value) { texCacheWarmed = value; },
			get texCompiling() { return texCompiling; }, set texCompiling(value) { texCompiling = value; },
			get texCompileQueued() { return texCompileQueued; }, set texCompileQueued(value) { texCompileQueued = value; },
			get isBusy() { return isBusy; }, set isBusy(value) { isBusy = value; },
			get texRevision() { return texRevision; }, set texRevision(value) { texRevision = value; },
			get draftBody() { return draftBody; },
			get activeSourceBytes() { return activeSourceBytes; }, set activeSourceBytes(value) { activeSourceBytes = value; },
			get sourceMaterialType() { return sourceMaterialType; }, set sourceMaterialType(value) { sourceMaterialType = value; },
			get showAttachedNote() { return showAttachedNote; }, set showAttachedNote(value) { showAttachedNote = value; },
			get texDiagnostics() { return texDiagnostics; }, set texDiagnostics(value) { texDiagnostics = value; },
			get latexDownloadMsg() { return latexDownloadMsg; }, set latexDownloadMsg(value) { latexDownloadMsg = value; },
			get activeSection() { return activeSection; }, set activeSection(value) { activeSection = value; },
			get sectionCache() { return sectionCache; }, set sectionCache(value) { sectionCache = value; },
			get workingDocType() { return workingDocType; },
			get lastTexBody() { return lastTexBody; }, set lastTexBody(value) { lastTexBody = value; },
			get texAutoCompile() { return texAutoCompile; },
			get texAutoTimer() { return texAutoTimer; }, set texAutoTimer(value) { texAutoTimer = value; },
			saveNote: () => saveNote(),
			get vditorInstance() { return vditorInstance; },
			get vditorContainer() { return vditorContainer; },
			get armedSelection() { return armedSelection; }, set armedSelection(value) { armedSelection = value; },
			get writeTargetNotice() { return writeTargetNotice; }, set writeTargetNotice(value) { writeTargetNotice = value; },
			get selDebounce() { return selDebounce; }, set selDebounce(value) { selDebounce = value; },
			get savedEditorRange() { return savedEditorRange; }, set savedEditorRange(value) { savedEditorRange = value; },
			get noteStreaming() { return noteStreaming; },
			triggerAutoSave,
			focusEditor,
			setSelectionSession: (value: any) => { selectionSession = value; },
			PANE_MIN_WIDTH,
			SIDEBAR_MIN_WIDTH,
			get splitRatio() { return splitRatio; }, set splitRatio(value) { splitRatio = value; },
			get isResizing() { return isResizing; }, set isResizing(value) { isResizing = value; },
			get mainLayoutEl() { return mainLayoutEl; },
			get sidebarWidth() { return sidebarWidth; }, set sidebarWidth(value) { sidebarWidth = value; },
			get isSidebarResizing() { return isSidebarResizing; }, set isSidebarResizing(value) { isSidebarResizing = value; },
			get activeSidebarTab() { return activeSidebarTab; }, set activeSidebarTab(value) { activeSidebarTab = value; },
			get chatTextareaEl() { return chatTextareaEl; },
			get chatMessagesEl() { return chatMessagesEl; },
			get chatMessages() { return chatMessages; },
			get userScrolledUp() { return userScrolledUp; }, set userScrolledUp(value) { userScrolledUp = value; },
			captureShortcutEditorTarget,
			restoreShortcutEditorFocus,
			tick
		});
		const {
			latexSession, pickLatexImage, compileTex, parseLatexError, closeTexPreview,
			getSelectionTextOffset, textOffsetOf, nearestIndexOf,
			computeSourceSelection, computeSourceCursor, clearArmedSelection, onDocMouseDown,
			captureEditorSelection, captureExternalTarget, reselectAfterEdit, armedEditTarget,
			onSelectionChange, handleGlobalSelectionChange, restoreSelectionTextOffset,
			saveCursorPosition, insertAtSavedCursor, mathSession, openMathDialog, insertMath,
			layoutSession, startSidebarResizing, startResizing, handleGlobalMouseMove,
			stopResizing, handleChatSidebarShortcut, handleChatScroll, scrollChatToBottom
		} = inputGraph;
		const graph = createNotePageGraph({
			createDocumentSession,
			createStreamingSession,
			createLinkingSession,
			createEditorSession,
			createNavigationSession,
			createSourceSession,
			createChatSession,
			get isLoadingNote() { return isLoadingNote; }, set isLoadingNote(value) { isLoadingNote = value; },
			get toolsReady() { return toolsReady; }, set toolsReady(value) { toolsReady = value; },
			clearArmedSelection, get writeTargetNotice() { return writeTargetNotice; }, set writeTargetNotice(value) { writeTargetNotice = value; },
			get chatPersistTimer() { return chatPersistTimer; }, set chatPersistTimer(value) { chatPersistTimer = value; },
			activeAiNoteId, destroyEditorInstance,
			get activeSourceBytes() { return activeSourceBytes; }, set activeSourceBytes(value) { activeSourceBytes = value; },
			get activeSourceId() { return activeSourceId; }, set activeSourceId(value) { activeSourceId = value; },
			get activeSection() { return activeSection; }, set activeSection(value) { activeSection = value; },
			get sectionCache() { return sectionCache; }, set sectionCache(value) { sectionCache = value; },
			get showAttachedNote() { return showAttachedNote; }, set showAttachedNote(value) { showAttachedNote = value; },
			get note() { return note; }, set note(value) { note = value; },
			get chatMessages() { return chatMessages; }, set chatMessages(value) { chatMessages = value; },
			get noteHistory() { return noteHistory; }, set noteHistory(value) { noteHistory = value; },
			get versionPreviewContent() { return versionPreviewContent; }, set versionPreviewContent(value) { versionPreviewContent = value; },
			get activeSidebarTab() { return activeSidebarTab; }, set activeSidebarTab(value) { activeSidebarTab = value; },
			get isSourceMaterial() { return isSourceMaterial; }, set isSourceMaterial(value) { isSourceMaterial = value; },
			get sourceMaterialType() { return sourceMaterialType; }, set sourceMaterialType(value) { sourceMaterialType = value; },
			get workingDocType() { return workingDocType; }, set workingDocType(value) { workingDocType = value; },
			get draftTitle() { return draftTitle; }, set draftTitle(value) { draftTitle = value; },
			get draftBody() { return draftBody; }, set draftBody(value) { draftBody = value; },
			get draftTags() { return draftTags; }, set draftTags(value) { draftTags = value; },
			get scratchpadSavedId() { return scratchpadSavedId; }, set scratchpadSavedId(value) { scratchpadSavedId = value; },
			get message() { return message; }, set message(value) { message = value; },
			openNoteNotebook, get saveStatus() { return saveStatus; }, set saveStatus(value) { saveStatus = value; },
			get vditorInstance() { return vditorInstance; }, get isBusy() { return isBusy; }, set isBusy(value) { isBusy = value; },
			goToHome: () => goto(resolve('/')), get saveNote() { return saveNote; }, setSaveNote: (value: () => Promise<void>) => { saveNote = value; },
			get noteStreamBackup() { return noteStreamBackup; }, set noteStreamBackup(value) { noteStreamBackup = value; },
			get noteStreamBuf() { return noteStreamBuf; }, set noteStreamBuf(value) { noteStreamBuf = value; },
			get noteStreaming() { return noteStreaming; }, set noteStreaming(value) { noteStreaming = value; },
			get transclusionObserver() { return transclusionObserver; }, get activeAiEditTarget() { return activeAiEditTarget; },
			get noteStreamSpan() { return noteStreamSpan; }, set noteStreamSpan(value) { noteStreamSpan = value; },
			get noteStreamFlushPending() { return noteStreamFlushPending; }, set noteStreamFlushPending(value) { noteStreamFlushPending = value; },
			getSelectionTextOffset, restoreSelectionTextOffset, get vditorContainer() { return vditorContainer; },
			get VditorConstructor() { return VditorConstructor; }, set VditorConstructor(value) { VditorConstructor = value; },
			localVditorCdn, get shouldRefocusEditor() { return shouldRefocusEditor; }, set shouldRefocusEditor(value) { shouldRefocusEditor = value; },
			insertAtSavedCursor, refocusEditorSoon, get vditorLoading() { return vditorLoading; }, set vditorLoading(value) { vditorLoading = value; },
			get shouldInitEditor() { return shouldInitEditor; }, get toolbarResizeObserver() { return toolbarResizeObserver; }, set toolbarResizeObserver(value) { toolbarResizeObserver = value; },
			get toolbarNeedsToggle() { return toolbarNeedsToggle; }, set toolbarNeedsToggle(value) { toolbarNeedsToggle = value; },
			get toolbarExpanded() { return toolbarExpanded; }, set toolbarExpanded(value) { toolbarExpanded = value; },
			get fullscreenShortcut() { return fullscreenShortcut; }, set fullscreenShortcut(value) { fullscreenShortcut = value; },
			get blockCache() { return blockCache; }, get relatedNotes() { return relatedNotes; }, set relatedNotes(value) { relatedNotes = value; },
			openMathDialog, saveCursorPosition, focusEditor, updateToolbarOverflow, triggerAutoSave,
			get versionPreviewHash() { return versionPreviewHash; }, set versionPreviewHash(value) { versionPreviewHash = value; },
			get versionPreviewDialog() { return versionPreviewDialog; }, get navigationWarningDialog() { return navigationWarningDialog; },
		hasReturnTo: () => page.url.searchParams.has('returnTo'), get backUrl() { return backUrl; },
			get pendingDebugTrace() { return pendingDebugTrace; }, set pendingDebugTrace(value) { pendingDebugTrace = value; },
			get debugInfo() { return debugInfo; }, set debugInfo(value) { debugInfo = value; }, get showDebugWindow() { return showDebugWindow; },
			get pdfIngestionStatus() { return pdfIngestionStatus; }, set pdfIngestionStatus(value) { pdfIngestionStatus = value; },
			get pdfIngestionError() { return pdfIngestionError; }, set pdfIngestionError(value) { pdfIngestionError = value; },
			get pdfIngestionPromise() { return pdfIngestionPromise; }, set pdfIngestionPromise(value) { pdfIngestionPromise = value; },
			get pdfSearchQuery() { return pdfSearchQuery; }, set pdfSearchQuery(value) { pdfSearchQuery = value; },
			get pdfSelectedIndex() { return pdfSelectedIndex; }, set pdfSelectedIndex(value) { pdfSelectedIndex = value; },
			get pdfNotesList() { return pdfNotesList; }, set pdfNotesList(value) { pdfNotesList = value; }, get filteredPdfs() { return filteredPdfs; },
			get attachPdfDialog() { return attachPdfDialog; }, get detachPdfDialog() { return detachPdfDialog; },
			tick, appendToNoteBody, get requireToolApproval() { return requireToolApproval; }, set requireToolApproval(value) { requireToolApproval = value; },
			get isChatStreaming() { return isChatStreaming; }, get activeAiComposerMode() { return activeAiComposerMode; }, set activeAiComposerMode(value) { activeAiComposerMode = value; },
			get activeChatNoteId() { return activeChatNoteId; }, set activeChatNoteId(value) { activeChatNoteId = value; }, get activeChatRequestId() { return activeChatRequestId; }, set activeChatRequestId(value) { activeChatRequestId = value; },
			get chatInput() { return chatInput; }, set chatInput(value) { chatInput = value; }, get chatTextareaEl() { return chatTextareaEl; }, get chatMessagesEl() { return chatMessagesEl; },
			get copiedIdx() { return copiedIdx; }, set copiedIdx(value) { copiedIdx = value; }, get chatChunkBuf() { return chatChunkBuf; }, set chatChunkBuf(value) { chatChunkBuf = value; },
			get chatChunkFlushPending() { return chatChunkFlushPending; }, set chatChunkFlushPending(value) { chatChunkFlushPending = value; },
			get MAX_DEBUG_MSG_CHARS() { return MAX_DEBUG_MSG_CHARS; }, get MAX_DEBUG_TRACE() { return MAX_DEBUG_TRACE; }, get approvalTimeouts() { return approvalTimeouts; },
			get armedSelection() { return armedSelection; },
			armedEditTarget, reselectAfterEdit, scrollChatToBottom, APPROVAL_TIMEOUT_MS
		});
		const {
			chatSession, documentSession, streamingSession, linkingSession, editorSession, navigationSession, sourceSession,
			persistChatHistory, checkpointChatHistory, flushChatChunks, loadCurrentNote, refreshCurrentNoteFromBackend,
			deleteNote, duplicateNote, beginNoteStream, scheduleNoteStreamFlush, flushNoteStream, appendNoteStream,
			cancelNoteStream, applyNoteWrite, initVditor, scanForTransclusions, setupTransclusionObserver, fetchRelatedNotes,
			fetchNoteHistory, previewVersion, restoreVersion, safeNavigate, goBack, navigateBack, handleBeforeUnload,
			confirmNavigation, cancelNavigation, handleSectionsReady, formatSectionCacheDuration, openAttachedNote,
			handlePdfQuote, handleAnnotationsChange, handleImageExtract, handlePdfTextExtracted, openAttachPdfDialog,
			attachPdf, requestDetachPdf, confirmDetachPdf, browseAndAttachPdf, handlePdfSearchKeydown, attachFile,
			persistableChatHistory, makeDebugTraceEntry, renderChatContent, setAiInteractionMode, handleActiveSectionChange,
			setToolApproval, setStreamingStatus, visibleAiStatus, copyMessage, stopActiveChat, stopChat, beginAiRequest,
			sendChatMessage, sendChatText, rewindToSnapshot, retryMessage, mergeChatTools, reconcileRequestNote,
			finishStreamingChatMessage, extractChatErrorMessage, failStreamingChatMessage, resolveApproval, aiEventContext
		} = graph;

		const lifecycle = createNotePageLifecycle({
			get aiInteractionMode() { return aiInteractionMode; },
			set aiInteractionMode(value) { aiInteractionMode = value; },
			PANE_MIN_WIDTH,
			SIDEBAR_MIN_WIDTH,
			get sidebarWidth() { return sidebarWidth; },
			set sidebarWidth(value) { sidebarWidth = value; },
			handleGlobalSelectionChange,
			onDocMouseDown,
			handleChatSidebarShortcut,
			aiEventContext,
			handleGlobalMouseMove,
			stopResizing,
			handleBeforeUnload,
			get chatPersistTimer() { return chatPersistTimer; },
			get texAutoTimer() { return texAutoTimer; },
			approvalTimeouts,
			activeAiNoteId,
			get chatMessages() { return chatMessages; },
			persistChatHistory,
			get toolbarResizeObserver() { return toolbarResizeObserver; },
			editorSession,
			sourceSession,
			get vditorInstance() { return vditorInstance; },
			get loadedRouteNoteId() { return loadedRouteNoteId; },
			set loadedRouteNoteId(value) { loadedRouteNoteId = value; },
			loadCurrentNote,
			get debugInfo() { return debugInfo; },
			set debugInfo(value) { debugInfo = value; },
			get showDebugWindow() { return showDebugWindow; },
			get activeChatRequestId() { return activeChatRequestId; },
			get debugTimer() { return debugTimer; },
			set debugTimer(value) { debugTimer = value; }
		});
	return {
		...lifecycle,
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
		...mathSession,
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
		pickLatexImage,
		compileTex,
		parseLatexError,
		closeTexPreview,
		parseBlocks: editorSession.parseBlocks,
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
		get pendingNavigationUrl() { return navigationSession.pendingNavigationUrl; },
		set pendingNavigationUrl(value) { navigationSession.pendingNavigationUrl = value; },
		get pendingBack() { return navigationSession.pendingBack; },
		set pendingBack(value) { navigationSession.pendingBack = value; },
		get isProgrammaticNavigation() { return navigationSession.isProgrammaticNavigation; },
		set isProgrammaticNavigation(value) { navigationSession.isProgrammaticNavigation = value; },
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
	};
}

export type NotePageController = ReturnType<typeof createNotePageController>;
