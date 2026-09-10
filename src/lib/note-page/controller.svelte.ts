import { invoke } from '@tauri-apps/api/core';
import { goto } from '$app/navigation';
import { resolve } from '$app/paths';
import { page } from '$app/state';
import type { NoteDocument, NoteSummary, GitCommit, ChatMessage } from '$lib/types';
import { tick, type Component, type SvelteComponent } from 'svelte';
import type Vditor from 'vditor';
import 'mathlive/fonts.css';
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
import { createNotePageUtilities } from './sessions/utilities.svelte';
import { createNotePageDialogs } from './sessions/dialogs.svelte';
import { createNotePageActions } from './sessions/actions.svelte';
import type { SelectionHandle } from '$lib/controller-context';
import { createBoundController } from '$lib/controllerView';
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
	let chatMessages = $state<ChatMessage[]>([]);
	let chatPersistenceError = $state<string | null>(null);
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
	let showDebugWindow = $state(false);
	try {
		showDebugWindow = localStorage.getItem('myelin_debug_window') === 'true';
	} catch (error) {
		message = 'Browser preferences could not be read; changes remain available for this session.';
		console.warn('Could not read debug-window preference', error);
	}
	$effect(() => {
		try {
			localStorage.setItem('myelin_debug_window', String(showDebugWindow));
		} catch (error) {
			message = 'Debug-window preference could not be saved; it will reset on the next launch.';
			console.warn('Could not save debug-window preference', error);
		}
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
	let VditorConstructor = $state<typeof Vditor | null>(null);
	let vditorLoading = $state(false);
	// Keep the note render separate from the editor/tool bundle. The bundle is
	// requested only after the note has had a chance to paint.
	let toolsReady = $state(false);
	let fullscreenShortcut = $state('Esc');
	// Pending tool-approval prompts auto-reject after this long if the user
	// never answers (mirrors the backend's TOOL_APPROVAL_TIMEOUT_SECS).
	const APPROVAL_TIMEOUT_MS = 120_000;
	// Timer bookkeeping is private controller state and is never rendered.
	// eslint-disable-next-line svelte/prefer-svelte-reactivity
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
	let PdfViewerComponent = $state<Component | null>(null);
	let EpubViewerComponent = $state<Component | null>(null);
	let HtmlViewerComponent = $state<Component | null>(null);
	let TexEditorComponent = $state<Component | null>(null);
	let IpynbEditorComponent = $state<Component | null>(null);
	let texEditorInstance: SvelteComponent | undefined = $state();
	let ipynbEditorInstance: SvelteComponent | undefined = $state();
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
	const PANE_MIN_WIDTH = 26 * 16;
	const SIDEBAR_MIN_WIDTH = 320;
	let sidebarWidth = $state(SIDEBAR_MIN_WIDTH);
	let isSidebarResizing = $state(false);
	let selectionSession: SelectionHandle;
	const editorInteractionPort = createBoundController(
		() => ({
			vditorInstance,
			vditorContainer,
			workingDocType,
			texEditorInstance,
			ipynbEditorInstance,
			shortcutEditorRange,
			shouldRefocusEditor
		}),
		{
			shortcutEditorRange: (value) => (shortcutEditorRange = value),
			shouldRefocusEditor: (value) => (shouldRefocusEditor = value)
		},
		{
			captureEditorSelection: () => selectionSession.captureEditorSelection()
		}
	);
	const editorInteraction = createEditorInteractionSession(editorInteractionPort);
	const {
		focusEditor,
		refocusEditorSoon,
		captureShortcutEditorTarget,
		restoreShortcutEditorFocus
	} = editorInteraction;
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
	let invalidateVditorInitialization = () => {};
	const editorStatePort = createBoundController(
		() => ({
			showAttachedNote,
			vditorInstance,
			draftBody,
			saveStatus,
			saveTimer
		}),
		{
			showAttachedNote: (value) => (showAttachedNote = value),
			vditorInstance: (value) => (vditorInstance = value),
			draftBody: (value) => (draftBody = value),
			saveStatus: (value) => (saveStatus = value),
			saveTimer: (value) => (saveTimer = value)
		},
		{
			invalidateVditorInitialization: () => invalidateVditorInitialization(),
			saveNote: () => saveNote()
		}
	);
	const editorState = createEditorStateSession(editorStatePort);
	const { appendToNoteBody, destroyEditorInstance, triggerAutoSave } = editorState;
	const utilitiesPort = createBoundController(
		() => ({
			note,
			isSourceMaterial,
			showAttachedNote,
			scratchpadSavedId,
			activeSourceId,
			sourceMaterialType,
			workingDocType,
			PdfViewerComponent,
			EpubViewerComponent,
			HtmlViewerComponent,
			TexEditorComponent,
			IpynbEditorComponent,
			vditorContainer,
			toolbarExpanded,
			toolbarNeedsToggle
		}),
		{
			PdfViewerComponent: (value) => (PdfViewerComponent = value),
			EpubViewerComponent: (value) => (EpubViewerComponent = value),
			HtmlViewerComponent: (value) => (HtmlViewerComponent = value),
			TexEditorComponent: (value) => (TexEditorComponent = value),
			IpynbEditorComponent: (value) => (IpynbEditorComponent = value),
			toolbarExpanded: (value) => (toolbarExpanded = value),
			toolbarNeedsToggle: (value) => (toolbarNeedsToggle = value)
		},
		{}
	);
	const utilities = createNotePageUtilities(utilitiesPort);
	const { openNoteNotebook, localVditorCdn, activeAiNoteId, updateToolbarOverflow } = utilities;

	const inputGraphPort = createBoundController(
		() => ({
			note,
			texCompileError,
			texPreviewStatus,
			texCacheWarmed,
			texCompiling,
			texCompileQueued,
			isBusy,
			texRevision,
			draftBody,
			activeSourceBytes,
			sourceMaterialType,
			showAttachedNote,
			texDiagnostics,
			latexDownloadMsg,
			activeSection,
			sectionCache,
			workingDocType,
			lastTexBody,
			texAutoCompile,
			texAutoTimer,
			vditorInstance,
			vditorContainer,
			armedSelection,
			writeTargetNotice,
			selDebounce,
			savedEditorRange,
			noteStreaming,
			splitRatio,
			isResizing,
			mainLayoutEl,
			sidebarWidth,
			isSidebarResizing,
			activeSidebarTab,
			chatTextareaEl,
			chatMessagesEl,
			chatMessages,
			chatPersistenceError,
			userScrolledUp
		}),
		{
			texCompileError: (value) => (texCompileError = value),
			texPreviewStatus: (value) => (texPreviewStatus = value),
			texCacheWarmed: (value) => (texCacheWarmed = value),
			texCompiling: (value) => (texCompiling = value),
			texCompileQueued: (value) => (texCompileQueued = value),
			isBusy: (value) => (isBusy = value),
			texRevision: (value) => (texRevision = value),
			activeSourceBytes: (value) => (activeSourceBytes = value),
			sourceMaterialType: (value) => (sourceMaterialType = value),
			showAttachedNote: (value) => (showAttachedNote = value),
			texDiagnostics: (value) => (texDiagnostics = value),
			latexDownloadMsg: (value) => (latexDownloadMsg = value),
			activeSection: (value) => (activeSection = value),
			sectionCache: (value) => (sectionCache = value),
			lastTexBody: (value) => (lastTexBody = value),
			texAutoTimer: (value) => (texAutoTimer = value),
			armedSelection: (value) => (armedSelection = value),
			writeTargetNotice: (value) => (writeTargetNotice = value),
			selDebounce: (value) => (selDebounce = value),
			savedEditorRange: (value) => (savedEditorRange = value),
			splitRatio: (value) => (splitRatio = value),
			isResizing: (value) => (isResizing = value),
			sidebarWidth: (value) => (sidebarWidth = value),
			isSidebarResizing: (value) => (isSidebarResizing = value),
			activeSidebarTab: (value) => (activeSidebarTab = value),
			chatPersistenceError: (value) => (chatPersistenceError = value),
			userScrolledUp: (value) => (userScrolledUp = value)
		},
		{
			saveNote: () => saveNote(),
			triggerAutoSave,
			focusEditor,
			setSelectionSession: (value: SelectionHandle) => {
				selectionSession = value;
			},
			PANE_MIN_WIDTH,
			SIDEBAR_MIN_WIDTH,
			captureShortcutEditorTarget,
			restoreShortcutEditorFocus,
			tick
		}
	);
	const inputGraph = createNoteInputGraph(inputGraphPort);
	const {
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
		handleGlobalSelectionChange,
		restoreSelectionTextOffset,
		saveCursorPosition,
		insertAtSavedCursor,
		mathSession,
		openMathDialog,
		startSidebarResizing,
		startResizing,
		handleGlobalMouseMove,
		stopResizing,
		handleChatSidebarShortcut,
		handleChatScroll,
		scrollChatToBottom
	} = inputGraph;
	const graphPort = createBoundController(
		() => ({
			aiInteractionMode,
			isLoadingNote,
			toolsReady,
			writeTargetNotice,
			chatPersistTimer,
			activeSourceBytes,
			activeSourceId,
			activeSection,
			sectionCache,
			showAttachedNote,
			note,
			chatMessages,
			chatPersistenceError,
			noteHistory,
			versionPreviewContent,
			activeSidebarTab,
			isSourceMaterial,
			sourceMaterialType,
			workingDocType,
			draftTitle,
			draftBody,
			draftTags,
			scratchpadSavedId,
			message,
			saveStatus,
			vditorInstance,
			isBusy,
			saveNote,
			noteStreamBackup,
			noteStreamBuf,
			noteStreaming,
			transclusionObserver,
			activeAiEditTarget,
			noteStreamSpan,
			noteStreamFlushPending,
			vditorContainer,
			VditorConstructor,
			shouldRefocusEditor,
			vditorLoading,
			shouldInitEditor,
			toolbarResizeObserver,
			toolbarNeedsToggle,
			toolbarExpanded,
			fullscreenShortcut,
			blockCache,
			relatedNotes,
			versionPreviewHash,
			versionPreviewDialog,
			navigationWarningDialog,
			backUrl,
			pendingDebugTrace,
			debugInfo,
			showDebugWindow,
			pdfIngestionStatus,
			pdfIngestionError,
			pdfIngestionPromise,
			pdfSearchQuery,
			pdfSelectedIndex,
			pdfNotesList,
			filteredPdfs,
			attachPdfDialog,
			detachPdfDialog,
			requireToolApproval,
			isChatStreaming,
			activeAiComposerMode,
			activeChatNoteId,
			activeChatRequestId,
			chatInput,
			chatTextareaEl,
			chatMessagesEl,
			copiedIdx,
			chatChunkBuf,
			chatChunkFlushPending,
			MAX_DEBUG_MSG_CHARS,
			MAX_DEBUG_TRACE,
			approvalTimeouts,
			armedSelection
		}),
		{
			isLoadingNote: (value) => (isLoadingNote = value),
			toolsReady: (value) => (toolsReady = value),
			writeTargetNotice: (value) => (writeTargetNotice = value),
			chatPersistTimer: (value) => (chatPersistTimer = value),
			activeSourceBytes: (value) => (activeSourceBytes = value),
			activeSourceId: (value) => (activeSourceId = value),
			activeSection: (value) => (activeSection = value),
			sectionCache: (value) => (sectionCache = value),
			showAttachedNote: (value) => (showAttachedNote = value),
			note: (value) => (note = value),
			chatMessages: (value) => (chatMessages = value),
			chatPersistenceError: (value) => (chatPersistenceError = value),
			noteHistory: (value) => (noteHistory = value),
			versionPreviewContent: (value) => (versionPreviewContent = value),
			activeSidebarTab: (value) => (activeSidebarTab = value),
			isSourceMaterial: (value) => (isSourceMaterial = value),
			sourceMaterialType: (value) => (sourceMaterialType = value),
			workingDocType: (value) => (workingDocType = value),
			draftTitle: (value) => (draftTitle = value),
			draftBody: (value) => (draftBody = value),
			draftTags: (value) => (draftTags = value),
			scratchpadSavedId: (value) => (scratchpadSavedId = value),
			message: (value) => (message = value),
			saveStatus: (value) => (saveStatus = value),
			vditorInstance: (value) => (vditorInstance = value),
			isBusy: (value) => (isBusy = value),
			noteStreamBackup: (value) => (noteStreamBackup = value),
			noteStreamBuf: (value) => (noteStreamBuf = value),
			noteStreaming: (value) => (noteStreaming = value),
			transclusionObserver: (value) => (transclusionObserver = value),
			activeAiEditTarget: (value) => (activeAiEditTarget = value),
			noteStreamSpan: (value) => (noteStreamSpan = value),
			noteStreamFlushPending: (value) => (noteStreamFlushPending = value),
			VditorConstructor: (value) => (VditorConstructor = value),
			shouldRefocusEditor: (value) => (shouldRefocusEditor = value),
			vditorLoading: (value) => (vditorLoading = value),
			toolbarResizeObserver: (value) => (toolbarResizeObserver = value),
			toolbarNeedsToggle: (value) => (toolbarNeedsToggle = value),
			toolbarExpanded: (value) => (toolbarExpanded = value),
			fullscreenShortcut: (value) => (fullscreenShortcut = value),
			relatedNotes: (value) => (relatedNotes = value),
			versionPreviewHash: (value) => (versionPreviewHash = value),
			pendingDebugTrace: (value) => (pendingDebugTrace = value),
			debugInfo: (value) => (debugInfo = value),
			pdfIngestionStatus: (value) => (pdfIngestionStatus = value),
			pdfIngestionError: (value) => (pdfIngestionError = value),
			pdfIngestionPromise: (value) => (pdfIngestionPromise = value),
			pdfSearchQuery: (value) => (pdfSearchQuery = value),
			pdfSelectedIndex: (value) => (pdfSelectedIndex = value),
			pdfNotesList: (value) => (pdfNotesList = value),
			requireToolApproval: (value) => (requireToolApproval = value),
			activeAiComposerMode: (value) => (activeAiComposerMode = value),
			activeChatNoteId: (value) => (activeChatNoteId = value),
			activeChatRequestId: (value) => (activeChatRequestId = value),
			chatInput: (value) => (chatInput = value),
			aiInteractionMode: (value) => (aiInteractionMode = value),
			copiedIdx: (value) => (copiedIdx = value),
			chatChunkBuf: (value) => (chatChunkBuf = value),
			chatChunkFlushPending: (value) => (chatChunkFlushPending = value)
		},
		{
			createDocumentSession,
			createStreamingSession,
			createLinkingSession,
			createEditorSession,
			createNavigationSession,
			createSourceSession,
			createChatSession,
			clearArmedSelection,
			activeAiNoteId,
			destroyEditorInstance,
			openNoteNotebook,
			goToHome: () => goto(resolve('/')),
			setSaveNote: (value: () => Promise<void>) => {
				saveNote = value;
			},
			getSelectionTextOffset,
			restoreSelectionTextOffset,
			localVditorCdn,
			insertAtSavedCursor,
			refocusEditorSoon,
			openMathDialog,
			saveCursorPosition,
			focusEditor,
			updateToolbarOverflow,
			triggerAutoSave,
			hasReturnTo: () => page.url.searchParams.has('returnTo'),
			tick,
			appendToNoteBody,
			// Keep the selected Chat/Write mode in the graph context. Without this
			// bridge the chat session sent `undefined`, and the backend accepted it
			// as legacy `auto` mode even while the Chat button looked active.
			get aiInteractionMode() {
				return aiInteractionMode;
			},
			armedEditTarget,
			reselectAfterEdit,
			scrollChatToBottom,
			APPROVAL_TIMEOUT_MS
		}
	);
	const graph = createNotePageGraph(graphPort);
	const {
		linkingSession,
		editorSession,
		navigationSession,
		sourceSession,
		persistChatHistory,
		checkpointChatHistory,
		flushChatChunks,
		loadCurrentNote,
		handleBeforeUnload,
		handleSectionsReady,
		formatSectionCacheDuration,
		openAttachedNote,
		handlePdfQuote,
		attachFile,
		persistableChatHistory,
		makeDebugTraceEntry,
		renderChatContent,
		setAiInteractionMode,
		handleActiveSectionChange,
		setToolApproval,
		setStreamingStatus,
		visibleAiStatus,
		copyMessage,
		aiEventContext
	} = graph;
	invalidateVditorInitialization = editorSession.invalidateInitialization;
	const dialogsPort = createBoundController(
		() => ({
			deleteMainNoteDialog,
			deleteAttachedNoteDialog,
			isSourceMaterial,
			scratchpadSavedId,
			activeSourceId,
			note,
			isBusy,
			saveTimer,
			draftBody,
			showAttachedNote,
			saveStatus,
			message
		}),
		{
			isBusy: (value) => (isBusy = value),
			saveTimer: (value) => (saveTimer = value),
			draftBody: (value) => (draftBody = value),
			showAttachedNote: (value) => (showAttachedNote = value),
			saveStatus: (value) => (saveStatus = value),
			message: (value) => (message = value)
		},
		{
			destroyEditorInstance,
			deleteNote: (noteId: string) => invoke('delete_note', { noteId }),
			navigationSession: graph.navigationSession,
			linkingSession: graph.linkingSession,
			goToNote: (noteId: string) => goto(resolve(`/notes/${encodeURIComponent(noteId)}`)),
			goToHref: (href: string) => {
				window.location.href = href;
			}
		}
	);
	const dialogs = createNotePageDialogs(dialogsPort);
	const { requestDeleteMainNote } = dialogs;
	const lifecyclePort = createBoundController(
		() => ({
			aiInteractionMode,
			sidebarWidth,
			chatPersistTimer,
			texAutoTimer,
			chatMessages,
			toolbarResizeObserver,
			vditorInstance,
			loadedRouteNoteId,
			debugInfo,
			showDebugWindow,
			activeChatRequestId,
			debugTimer
		}),
		{
			aiInteractionMode: (value) => (aiInteractionMode = value),
			sidebarWidth: (value) => (sidebarWidth = value),
			loadedRouteNoteId: (value) => (loadedRouteNoteId = value),
			debugInfo: (value) => (debugInfo = value),
			debugTimer: (value) => (debugTimer = value)
		},
		{
			PANE_MIN_WIDTH,
			SIDEBAR_MIN_WIDTH,
			handleGlobalSelectionChange,
			onDocMouseDown,
			handleChatSidebarShortcut,
			aiEventContext,
			handleGlobalMouseMove,
			stopResizing,
			handleBeforeUnload,
			approvalTimeouts,
			activeAiNoteId,
			persistChatHistory,
			editorSession,
			sourceSession,
			destroyEditorInstance,
			loadCurrentNote
		}
	);
	const lifecycle = createNotePageLifecycle(lifecyclePort);
	return createBoundController(
		() => ({
			PdfViewerComponent,
			EpubViewerComponent,
			HtmlViewerComponent,
			TexEditorComponent,
			IpynbEditorComponent,
			shouldRenderEditor,
			currentTime,
			...editorInteractionPort,
			...editorStatePort,
			...inputGraphPort,
			...graphPort,
			...dialogsPort,
			...lifecyclePort,
			loadedRouteNoteId
		}),
		{
			requireToolApproval: (value) => (requireToolApproval = value),
			note: (value) => (note = value),
			isLoadingNote: (value) => (isLoadingNote = value),
			draftBody: (value) => (draftBody = value),
			draftTitle: (value) => (draftTitle = value),
			draftTags: (value) => (draftTags = value),
			isBusy: (value) => (isBusy = value),
			message: (value) => (message = value),
			latexDownloadMsg: (value) => (latexDownloadMsg = value),
			texAutoCompile: (value) => (texAutoCompile = value),
			texCompiling: (value) => (texCompiling = value),
			texCacheWarmed: (value) => (texCacheWarmed = value),
			texPreviewStatus: (value) => (texPreviewStatus = value),
			texCompileError: (value) => (texCompileError = value),
			texRevision: (value) => (texRevision = value),
			texCompileQueued: (value) => (texCompileQueued = value),
			lastTexBody: (value) => (lastTexBody = value),
			texDiagnostics: (value) => (texDiagnostics = value),
			texAutoTimer: (value) => (texAutoTimer = value),
			activeSidebarTab: (value) => (activeSidebarTab = value),
			noteHistory: (value) => (noteHistory = value),
			versionPreviewContent: (value) => (versionPreviewContent = value),
			versionPreviewHash: (value) => (versionPreviewHash = value),
			versionPreviewDialog: (value) => (versionPreviewDialog = value),
			chatMessages: (value) => (chatMessages = value),
			chatPersistenceError: (value) => (chatPersistenceError = value),
			chatInput: (value) => (chatInput = value),
			copiedIdx: (value) => (copiedIdx = value),
			chatChunkBuf: (value) => (chatChunkBuf = value),
			chatChunkFlushPending: (value) => (chatChunkFlushPending = value),
			chatPersistTimer: (value) => (chatPersistTimer = value),
			showDebugWindow: (value) => (showDebugWindow = value),
			pendingDebugTrace: (value) => (pendingDebugTrace = value),
			activeAiComposerMode: (value) => (activeAiComposerMode = value),
			activeChatNoteId: (value) => (activeChatNoteId = value),
			aiInteractionMode: (value) => (aiInteractionMode = value),
			debugInfo: (value) => (debugInfo = value),
			armedSelection: (value) => (armedSelection = value),
			selDebounce: (value) => (selDebounce = value),
			activeAiEditTarget: (value) => (activeAiEditTarget = value),
			writeTargetNotice: (value) => (writeTargetNotice = value),
			activeChatRequestId: (value) => (activeChatRequestId = value),
			isChatStreaming: (value) => (isChatStreaming = value),
			chatTextareaEl: (value) => (chatTextareaEl = value),
			chatMessagesEl: (value) => (chatMessagesEl = value),
			currentTime: (value) => (currentTime = value),
			debugTimer: (value) => (debugTimer = value),
			backUrl: (value) => (backUrl = value),
			relatedNotes: (value) => (relatedNotes = value),
			vditorContainer: (value) => (vditorContainer = value),
			vditorInstance: (value) => (vditorInstance = value),
			VditorConstructor: (value) => (VditorConstructor = value),
			vditorLoading: (value) => (vditorLoading = value),
			toolsReady: (value) => (toolsReady = value),
			fullscreenShortcut: (value) => (fullscreenShortcut = value),
			noteStreaming: (value) => (noteStreaming = value),
			noteStreamBuf: (value) => (noteStreamBuf = value),
			noteStreamBackup: (value) => (noteStreamBackup = value),
			noteStreamFlushPending: (value) => (noteStreamFlushPending = value),
			noteStreamSpan: (value) => (noteStreamSpan = value),
			savedEditorRange: (value) => (savedEditorRange = value),
			shortcutEditorRange: (value) => (shortcutEditorRange = value),
			shouldRefocusEditor: (value) => (shouldRefocusEditor = value),
			isSourceMaterial: (value) => (isSourceMaterial = value),
			sourceMaterialType: (value) => (sourceMaterialType = value),
			workingDocType: (value) => (workingDocType = value),
			PdfViewerComponent: (value) => (PdfViewerComponent = value),
			EpubViewerComponent: (value) => (EpubViewerComponent = value),
			HtmlViewerComponent: (value) => (HtmlViewerComponent = value),
			TexEditorComponent: (value) => (TexEditorComponent = value),
			IpynbEditorComponent: (value) => (IpynbEditorComponent = value),
			texEditorInstance: (value) => (texEditorInstance = value),
			ipynbEditorInstance: (value) => (ipynbEditorInstance = value),
			activeSourceId: (value) => (activeSourceId = value),
			activeSourceBytes: (value) => (activeSourceBytes = value),
			activeSection: (value) => (activeSection = value),
			sectionCache: (value) => (sectionCache = value),
			scratchpadSavedId: (value) => (scratchpadSavedId = value),
			showAttachedNote: (value) => (showAttachedNote = value),
			pdfIngestionStatus: (value) => (pdfIngestionStatus = value),
			pdfIngestionError: (value) => (pdfIngestionError = value),
			pdfIngestionPromise: (value) => (pdfIngestionPromise = value),
			splitRatio: (value) => (splitRatio = value),
			isResizing: (value) => (isResizing = value),
			mainLayoutEl: (value) => (mainLayoutEl = value),
			sidebarWidth: (value) => (sidebarWidth = value),
			isSidebarResizing: (value) => (isSidebarResizing = value),
			userScrolledUp: (value) => (userScrolledUp = value),
			blockCache: (value) => (blockCache = value),
			transclusionObserver: (value) => (transclusionObserver = value),
			toolbarExpanded: (value) => (toolbarExpanded = value),
			toolbarNeedsToggle: (value) => (toolbarNeedsToggle = value),
			toolbarResizeObserver: (value) => (toolbarResizeObserver = value),
			saveStatus: (value) => (saveStatus = value),
			saveTimer: (value) => (saveTimer = value),
			navigationWarningDialog: (value) => (navigationWarningDialog = value),
			deleteAttachedNoteDialog: (value) => (deleteAttachedNoteDialog = value),
			deleteMainNoteDialog: (value) => (deleteMainNoteDialog = value),
			detachPdfDialog: (value) => (detachPdfDialog = value),
			attachPdfDialog: (value) => (attachPdfDialog = value),
			pdfSearchQuery: (value) => (pdfSearchQuery = value),
			pdfNotesList: (value) => (pdfNotesList = value),
			pdfSelectedIndex: (value) => (pdfSelectedIndex = value),
			filteredPdfs: (value) => (filteredPdfs = value),
			shouldRenderEditor: (value) => (shouldRenderEditor = value),
			shouldInitEditor: (value) => (shouldInitEditor = value),
			loadedRouteNoteId: (value) => (loadedRouteNoteId = value)
		},
		{
			...lifecycle,
			persistableChatHistory,
			persistChatHistory,
			checkpointChatHistory,
			flushChatChunks,
			makeDebugTraceEntry,
			renderChatContent,
			setAiInteractionMode,
			handleActiveSectionChange,
			setToolApproval,
			setStreamingStatus,
			visibleAiStatus,
			copyMessage,
			openNoteNotebook,
			attachFile,
			localVditorCdn,
			activeAiNoteId,
			handleSectionsReady,
			formatSectionCacheDuration,
			openAttachedNote,
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
			requestDeleteMainNote,
			...createNotePageActions({
				...inputGraph,
				...graph,
				...dialogs,
				editorSession,
				navigationSession,
				linkingSession,
				saveNote,
				updateToolbarOverflow
			})
		}
	);
}
export type NotePageController = ReturnType<typeof createNotePageController>;
