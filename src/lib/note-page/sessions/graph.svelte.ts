import { createDocumentSession } from './graph/document';
import { createLinkingSession } from './graph/linking';
import type { ControllerContext } from '$lib/controller-context';
import type { createChatSession } from './chat.svelte';
import type { createEditorSession } from './editor.svelte';
import type { createNavigationSession } from './navigation.svelte';
import type { createSourceSession } from './source.svelte';
import type { createStreamingSession } from './streaming.svelte';
import type { createLinkingSession as createLinkingSessionType } from './graph/linking';
import type { createDocumentSession as createDocumentSessionType } from './graph/document';

type NotePageGraphContext = ControllerContext;
type EditorSession = ReturnType<typeof createEditorSession>;
type NavigationSession = ReturnType<typeof createNavigationSession>;
type SourceSession = ReturnType<typeof createSourceSession>;
type ChatSession = ReturnType<typeof createChatSession>;
type StreamingSession = ReturnType<typeof createStreamingSession>;
type LinkingSession = ReturnType<typeof createLinkingSessionType>;
type DocumentSession = ReturnType<typeof createDocumentSessionType>;

/** Creates the cross-session graph for the note page. */
export function createNotePageGraph(rawContext: object) {
	const ctx = rawContext as NotePageGraphContext;
	// The graph has initialization cycles between navigation, source, editor, and chat.
	// eslint-disable-next-line prefer-const
	let editorSession: EditorSession;
	// eslint-disable-next-line prefer-const
	let navigationSession: NavigationSession;
	// eslint-disable-next-line prefer-const
	let sourceSession: SourceSession;
	// eslint-disable-next-line prefer-const
	let chatSession: ChatSession;

	const persistChatHistory = (...args: Parameters<ChatSession['persistChatHistory']>) =>
		chatSession.persistChatHistory(...args);
	const checkpointChatHistory = (...args: Parameters<ChatSession['checkpointChatHistory']>) =>
		chatSession.checkpointChatHistory(...args);
	const flushChatChunks = () => chatSession.flushChatChunks();

	const documentSession = createDocumentSession(
		ctx,
		persistChatHistory,
		() => navigationSession,
		() => editorSession
	) as DocumentSession;

	ctx.setSaveNote(documentSession.saveNote);

	const streamingSession = (ctx.createStreamingSession as (context: object) => unknown)({
		get noteStreamBackup() {
			return ctx.noteStreamBackup;
		},
		set noteStreamBackup(value) {
			ctx.noteStreamBackup = value;
		},
		get noteStreamBuf() {
			return ctx.noteStreamBuf;
		},
		set noteStreamBuf(value) {
			ctx.noteStreamBuf = value;
		},
		get noteStreaming() {
			return ctx.noteStreaming;
		},
		set noteStreaming(value) {
			ctx.noteStreaming = value;
		},
		get transclusionObserver() {
			return ctx.transclusionObserver;
		},
		get activeAiEditTarget() {
			return ctx.activeAiEditTarget;
		},
		get noteStreamSpan() {
			return ctx.noteStreamSpan;
		},
		set noteStreamSpan(value) {
			ctx.noteStreamSpan = value;
		},
		get noteStreamFlushPending() {
			return ctx.noteStreamFlushPending;
		},
		set noteStreamFlushPending(value) {
			ctx.noteStreamFlushPending = value;
		},
		get draftBody() {
			return ctx.draftBody;
		},
		set draftBody(value) {
			ctx.draftBody = value;
		},
		get vditorInstance() {
			return ctx.vditorInstance;
		},
		get vditorContainer() {
			return ctx.vditorContainer;
		},
		get note() {
			return ctx.note;
		},
		set note(value) {
			ctx.note = value;
		},
		getSelectionTextOffset: ctx.getSelectionTextOffset,
		restoreSelectionTextOffset: ctx.restoreSelectionTextOffset,
		setupTransclusionObserver: () => editorSession.setupTransclusionObserver()
	}) as StreamingSession;

	const linkingSession = createLinkingSession(ctx) as LinkingSession;

	editorSession = (ctx.createEditorSession as (context: object) => unknown)({
		get VditorConstructor() {
			return ctx.VditorConstructor;
		},
		set VditorConstructor(value) {
			ctx.VditorConstructor = value;
		},
		get vditorContainer() {
			return ctx.vditorContainer;
		},
		get vditorInstance() {
			return ctx.vditorInstance;
		},
		set vditorInstance(value) {
			ctx.vditorInstance = value;
		},
		get vditorLoading() {
			return ctx.vditorLoading;
		},
		set vditorLoading(value) {
			ctx.vditorLoading = value;
		},
		get toolsReady() {
			return ctx.toolsReady;
		},
		get shouldInitEditor() {
			return ctx.shouldInitEditor;
		},
		get draftBody() {
			return ctx.draftBody;
		},
		set draftBody(value) {
			ctx.draftBody = value;
		},
		get isSourceMaterial() {
			return ctx.isSourceMaterial;
		},
		get message() {
			return ctx.message;
		},
		set message(value) {
			ctx.message = value;
		},
		get toolbarResizeObserver() {
			return ctx.toolbarResizeObserver;
		},
		set toolbarResizeObserver(value) {
			ctx.toolbarResizeObserver = value;
		},
		get toolbarNeedsToggle() {
			return ctx.toolbarNeedsToggle;
		},
		set toolbarNeedsToggle(value) {
			ctx.toolbarNeedsToggle = value;
		},
		get toolbarExpanded() {
			return ctx.toolbarExpanded;
		},
		set toolbarExpanded(value) {
			ctx.toolbarExpanded = value;
		},
		get fullscreenShortcut() {
			return ctx.fullscreenShortcut;
		},
		set fullscreenShortcut(value) {
			ctx.fullscreenShortcut = value;
		},
		get blockCache() {
			return ctx.blockCache;
		},
		get transclusionObserver() {
			return ctx.transclusionObserver;
		},
		set transclusionObserver(value) {
			ctx.transclusionObserver = value;
		},
		get draftTags() {
			return ctx.draftTags;
		},
		get relatedNotes() {
			return ctx.relatedNotes;
		},
		set relatedNotes(value) {
			ctx.relatedNotes = value;
		},
		get note() {
			return ctx.note;
		},
		localVditorCdn: ctx.localVditorCdn,
		openAttachPdfDialog: () => sourceSession.openAttachPdfDialog(),
		openMathDialog: ctx.openMathDialog,
		openLinkDialog: () => {
			ctx.saveCursorPosition();
			linkingSession.linkSearchQuery = '';
			linkingSession.linkSearchResults = [];
			linkingSession.linkNoteDialog?.showModal();
			setTimeout(() => {
				const input = linkingSession.linkNoteDialog?.querySelector(
					'.link-search-input'
				) as HTMLInputElement;
				input?.focus();
			}, 50);
		},
		linkingSession,
		updateToolbarOverflow: ctx.updateToolbarOverflow,
		triggerAutoSave: ctx.triggerAutoSave
	}) as EditorSession;

	navigationSession = (ctx.createNavigationSession as (context: object) => unknown)({
		get note() {
			return ctx.note;
		},
		get isBusy() {
			return ctx.isBusy;
		},
		set isBusy(value) {
			ctx.isBusy = value;
		},
		get noteHistory() {
			return ctx.noteHistory;
		},
		set noteHistory(value) {
			ctx.noteHistory = value;
		},
		get versionPreviewContent() {
			return ctx.versionPreviewContent;
		},
		set versionPreviewContent(value) {
			ctx.versionPreviewContent = value;
		},
		get versionPreviewHash() {
			return ctx.versionPreviewHash;
		},
		set versionPreviewHash(value) {
			ctx.versionPreviewHash = value;
		},
		get versionPreviewDialog() {
			return ctx.versionPreviewDialog;
		},
		get draftBody() {
			return ctx.draftBody;
		},
		set draftBody(value) {
			ctx.draftBody = value;
		},
		get vditorInstance() {
			return ctx.vditorInstance;
		},
		get activeSidebarTab() {
			return ctx.activeSidebarTab;
		},
		set activeSidebarTab(value) {
			ctx.activeSidebarTab = value;
		},
		get navigationWarningDialog() {
			return ctx.navigationWarningDialog;
		},
		get saveStatus() {
			return ctx.saveStatus;
		},
		hasReturnTo: ctx.hasReturnTo,
		get backUrl() {
			return ctx.backUrl;
		},
		triggerAutoSave: ctx.triggerAutoSave
	}) as NavigationSession;

	sourceSession = (ctx.createSourceSession as (context: object) => unknown)({
		get note() {
			return ctx.note;
		},
		set note(value) {
			ctx.note = value;
		},
		get isSourceMaterial() {
			return ctx.isSourceMaterial;
		},
		activeAiNoteId: ctx.activeAiNoteId,
		get activeSection() {
			return ctx.activeSection;
		},
		get sectionCache() {
			return ctx.sectionCache;
		},
		set sectionCache(value) {
			ctx.sectionCache = value;
		},
		get scratchpadSavedId() {
			return ctx.scratchpadSavedId;
		},
		set scratchpadSavedId(value) {
			ctx.scratchpadSavedId = value;
		},
		get draftTitle() {
			return ctx.draftTitle;
		},
		set draftTitle(value) {
			ctx.draftTitle = value;
		},
		get draftBody() {
			return ctx.draftBody;
		},
		set draftBody(value) {
			ctx.draftBody = value;
		},
		get draftTags() {
			return ctx.draftTags;
		},
		set draftTags(value) {
			ctx.draftTags = value;
		},
		get chatMessages() {
			return ctx.chatMessages;
		},
		set chatMessages(value) {
			ctx.chatMessages = value;
		},
		get showAttachedNote() {
			return ctx.showAttachedNote;
		},
		set showAttachedNote(value) {
			ctx.showAttachedNote = value;
		},
		get activeSourceId() {
			return ctx.activeSourceId;
		},
		set activeSourceId(value) {
			ctx.activeSourceId = value;
		},
		get activeSourceBytes() {
			return ctx.activeSourceBytes;
		},
		set activeSourceBytes(value) {
			ctx.activeSourceBytes = value;
		},
		get sourceMaterialType() {
			return ctx.sourceMaterialType;
		},
		set sourceMaterialType(value) {
			ctx.sourceMaterialType = value;
		},
		set activeSection(value) {
			ctx.activeSection = value;
		},
		get pendingDebugTrace() {
			return ctx.pendingDebugTrace;
		},
		set pendingDebugTrace(value) {
			ctx.pendingDebugTrace = value;
		},
		get debugInfo() {
			return ctx.debugInfo;
		},
		set debugInfo(value) {
			ctx.debugInfo = value;
		},
		get showDebugWindow() {
			return ctx.showDebugWindow;
		},
		get pdfIngestionStatus() {
			return ctx.pdfIngestionStatus;
		},
		set pdfIngestionStatus(value) {
			ctx.pdfIngestionStatus = value;
		},
		get pdfIngestionError() {
			return ctx.pdfIngestionError;
		},
		set pdfIngestionError(value) {
			ctx.pdfIngestionError = value;
		},
		get pdfIngestionPromise() {
			return ctx.pdfIngestionPromise;
		},
		set pdfIngestionPromise(value) {
			ctx.pdfIngestionPromise = value;
		},
		get pdfSearchQuery() {
			return ctx.pdfSearchQuery;
		},
		set pdfSearchQuery(value) {
			ctx.pdfSearchQuery = value;
		},
		get pdfSelectedIndex() {
			return ctx.pdfSelectedIndex;
		},
		set pdfSelectedIndex(value) {
			ctx.pdfSelectedIndex = value;
		},
		get pdfNotesList() {
			return ctx.pdfNotesList;
		},
		set pdfNotesList(value) {
			ctx.pdfNotesList = value;
		},
		get filteredPdfs() {
			return ctx.filteredPdfs;
		},
		get attachPdfDialog() {
			return ctx.attachPdfDialog;
		},
		get detachPdfDialog() {
			return ctx.detachPdfDialog;
		},
		get message() {
			return ctx.message;
		},
		set message(value) {
			ctx.message = value;
		},
		get isBusy() {
			return ctx.isBusy;
		},
		set isBusy(value) {
			ctx.isBusy = value;
		},
		get saveStatus() {
			return ctx.saveStatus;
		},
		set saveStatus(value) {
			ctx.saveStatus = value;
		},
		openNoteNotebook: ctx.openNoteNotebook,
		tick: ctx.tick,
		appendToNoteBody: ctx.appendToNoteBody,
		triggerAutoSave: ctx.triggerAutoSave,
		destroyEditorInstance: ctx.destroyEditorInstance,
		initVditor: () => editorSession.initVditor()
	}) as SourceSession;

	const chatContext = {
		get activeChatRequestId() {
			return ctx.activeChatRequestId;
		},
		set activeChatRequestId(value) {
			ctx.activeChatRequestId = value;
		},
		get isChatStreaming() {
			return ctx.isChatStreaming;
		},
		get pendingDebugTrace() {
			return ctx.pendingDebugTrace;
		},
		set pendingDebugTrace(value) {
			ctx.pendingDebugTrace = value;
		},
		get debugInfo() {
			return ctx.debugInfo;
		},
		set debugInfo(value) {
			ctx.debugInfo = value;
		},
		get showDebugWindow() {
			return ctx.showDebugWindow;
		},
		get chatMessages() {
			return ctx.chatMessages;
		},
		set chatMessages(value) {
			ctx.chatMessages = value;
		},
		get activeAiComposerMode() {
			return ctx.activeAiComposerMode;
		},
		set activeAiComposerMode(value) {
			ctx.activeAiComposerMode = value;
		},
		get activeChatNoteId() {
			return ctx.activeChatNoteId;
		},
		set activeChatNoteId(value) {
			ctx.activeChatNoteId = value;
		},
		get chatInput() {
			return ctx.chatInput;
		},
		set chatInput(value) {
			ctx.chatInput = value;
		},
		get chatTextareaEl() {
			return ctx.chatTextareaEl;
		},
		get note() {
			return ctx.note;
		},
		set note(value) {
			ctx.note = value;
		},
		get aiInteractionMode() {
			return ctx.aiInteractionMode;
		},
		set aiInteractionMode(value) {
			ctx.aiInteractionMode = value;
		},
		get activeSection() {
			return ctx.activeSection;
		},
		set activeSection(value) {
			ctx.activeSection = value;
		},
		get requireToolApproval() {
			return ctx.requireToolApproval;
		},
		set requireToolApproval(value) {
			ctx.requireToolApproval = value;
		},
		get writeTargetNotice() {
			return ctx.writeTargetNotice;
		},
		set writeTargetNotice(value) {
			ctx.writeTargetNotice = value;
		},
		get isSourceMaterial() {
			return ctx.isSourceMaterial;
		},
		get showAttachedNote() {
			return ctx.showAttachedNote;
		},
		get saveStatus() {
			return ctx.saveStatus;
		},
		get pdfIngestionPromise() {
			return ctx.pdfIngestionPromise;
		},
		get draftBody() {
			return ctx.draftBody;
		},
		set draftBody(value) {
			ctx.draftBody = value;
		},
		get draftTitle() {
			return ctx.draftTitle;
		},
		set draftTitle(value) {
			ctx.draftTitle = value;
		},
		get draftTags() {
			return ctx.draftTags;
		},
		set draftTags(value) {
			ctx.draftTags = value;
		},
		get workingDocType() {
			return ctx.workingDocType;
		},
		get activeSourceId() {
			return ctx.activeSourceId;
		},
		get vditorInstance() {
			return ctx.vditorInstance;
		},
		get chatMessagesEl() {
			return ctx.chatMessagesEl;
		},
		get copiedIdx() {
			return ctx.copiedIdx;
		},
		set copiedIdx(value) {
			ctx.copiedIdx = value;
		},
		get chatChunkBuf() {
			return ctx.chatChunkBuf;
		},
		set chatChunkBuf(value) {
			ctx.chatChunkBuf = value;
		},
		get MAX_DEBUG_MSG_CHARS() {
			return ctx.MAX_DEBUG_MSG_CHARS;
		},
		get isBusy() {
			return ctx.isBusy;
		},
		set isBusy(value) {
			ctx.isBusy = value;
		},
		get chatPersistTimer() {
			return ctx.chatPersistTimer;
		},
		set chatPersistTimer(value) {
			ctx.chatPersistTimer = value;
		},
		get noteStreaming() {
			return ctx.noteStreaming;
		},
		get activeAiEditTarget() {
			return ctx.activeAiEditTarget;
		},
		set activeAiEditTarget(value) {
			ctx.activeAiEditTarget = value;
		},
		get approvalTimeouts() {
			return ctx.approvalTimeouts;
		},
		activeAiNoteId: ctx.activeAiNoteId,
		armedEditTarget: ctx.armedEditTarget,
		tick: ctx.tick,
		saveNote: ctx.saveNote,
		checkpointChatHistory,
		persistChatHistory,
		flushChatChunks,
		scrollChatToBottom: ctx.scrollChatToBottom,
		cancelNoteStream: streamingSession.cancelNoteStream,
		fetchRelatedNotes: editorSession.fetchRelatedNotes,
		beginNoteStream: streamingSession.beginNoteStream,
		appendNoteStream: streamingSession.appendNoteStream,
		applyNoteWrite: streamingSession.applyNoteWrite
	} as object & {
		failStreamingChatMessage?: (requestId: string, error: string) => void;
	};
	chatSession = (ctx.createChatSession as (context: object) => unknown)(chatContext) as ChatSession;
	chatContext.failStreamingChatMessage = chatSession.failStreamingChatMessage;

	const aiEventContext: object = {
		get note() {
			return ctx.note;
		},
		set note(value) {
			ctx.note = value;
		},
		get message() {
			return ctx.message;
		},
		set message(value) {
			ctx.message = value;
		},
		get latexDownloadMsg() {
			return ctx.latexDownloadMsg;
		},
		set latexDownloadMsg(value) {
			ctx.latexDownloadMsg = value;
		},
		get texCacheWarmed() {
			return ctx.texCacheWarmed;
		},
		set texCacheWarmed(value) {
			ctx.texCacheWarmed = value;
		},
		get chatMessages() {
			return ctx.chatMessages;
		},
		set chatMessages(value) {
			ctx.chatMessages = value;
		},
		get chatChunkBuf() {
			return ctx.chatChunkBuf;
		},
		set chatChunkBuf(value) {
			ctx.chatChunkBuf = value;
		},
		get chatChunkFlushPending() {
			return ctx.chatChunkFlushPending;
		},
		set chatChunkFlushPending(value) {
			ctx.chatChunkFlushPending = value;
		},
		get showDebugWindow() {
			return ctx.showDebugWindow;
		},
		get pendingDebugTrace() {
			return ctx.pendingDebugTrace;
		},
		set pendingDebugTrace(value) {
			ctx.pendingDebugTrace = value;
		},
		get activeAiComposerMode() {
			return ctx.activeAiComposerMode;
		},
		get debugInfo() {
			return ctx.debugInfo;
		},
		set debugInfo(value) {
			ctx.debugInfo = value;
		},
		get armedSelection() {
			return ctx.armedSelection;
		},
		get activeChatRequestId() {
			return ctx.activeChatRequestId;
		},
		get chatMessagesEl() {
			return ctx.chatMessagesEl;
		},
		get workingDocType() {
			return ctx.workingDocType;
		},
		get sectionCache() {
			return ctx.sectionCache;
		},
		set sectionCache(value) {
			ctx.sectionCache = value;
		},
		APPROVAL_TIMEOUT_MS: ctx.APPROVAL_TIMEOUT_MS,
		approvalTimeouts: ctx.approvalTimeouts,
		MAX_DEBUG_TRACE: ctx.MAX_DEBUG_TRACE,
		activeAiNoteId: ctx.activeAiNoteId,
		flushChatChunks,
		makeDebugTraceEntry: chatSession.makeDebugTraceEntry,
		setStreamingStatus: chatSession.setStreamingStatus,
		visibleAiStatus: chatSession.visibleAiStatus,
		scrollChatToBottom: ctx.scrollChatToBottom,
		clearArmedSelection: ctx.clearArmedSelection,
		reselectAfterEdit: ctx.reselectAfterEdit,
		beginNoteStream: streamingSession.beginNoteStream,
		appendNoteStream: streamingSession.appendNoteStream,
		cancelNoteStream: streamingSession.cancelNoteStream,
		applyNoteWrite: streamingSession.applyNoteWrite,
		finishStreamingChatMessage: chatSession.finishStreamingChatMessage,
		failStreamingChatMessage: chatSession.failStreamingChatMessage,
		resolveApproval: chatSession.resolveApproval
	};

	return {
		chatSession,
		documentSession,
		streamingSession,
		linkingSession,
		editorSession,
		navigationSession,
		sourceSession,
		persistChatHistory,
		checkpointChatHistory,
		flushChatChunks,
		loadCurrentNote: documentSession.loadCurrentNote,
		refreshCurrentNoteFromBackend: documentSession.refreshCurrentNoteFromBackend,
		deleteNote: documentSession.deleteCurrent,
		duplicateNote: documentSession.duplicateCurrent,
		beginNoteStream: streamingSession.beginNoteStream,
		scheduleNoteStreamFlush: streamingSession.scheduleNoteStreamFlush,
		flushNoteStream: streamingSession.flushNoteStream,
		appendNoteStream: streamingSession.appendNoteStream,
		cancelNoteStream: streamingSession.cancelNoteStream,
		applyNoteWrite: streamingSession.applyNoteWrite,
		initVditor: editorSession.initVditor,
		scanForTransclusions: editorSession.scanForTransclusions,
		setupTransclusionObserver: editorSession.setupTransclusionObserver,
		fetchRelatedNotes: editorSession.fetchRelatedNotes,
		fetchNoteHistory: navigationSession.fetchNoteHistory,
		previewVersion: navigationSession.previewVersion,
		restoreVersion: navigationSession.restoreVersion,
		safeNavigate: navigationSession.safeNavigate,
		goBack: navigationSession.goBack,
		navigateBack: navigationSession.navigateBack,
		handleBeforeUnload: navigationSession.handleBeforeUnload,
		confirmNavigation: navigationSession.confirmNavigation,
		cancelNavigation: navigationSession.cancelNavigation,
		handleSectionsReady: sourceSession.handleSectionsReady,
		formatSectionCacheDuration: sourceSession.formatSectionCacheDuration,
		openAttachedNote: sourceSession.openAttachedNote,
		handlePdfQuote: sourceSession.handlePdfQuote,
		handleAnnotationsChange: sourceSession.handleAnnotationsChange,
		handleImageExtract: sourceSession.handleImageExtract,
		handlePdfTextExtracted: sourceSession.handlePdfTextExtracted,
		openAttachPdfDialog: sourceSession.openAttachPdfDialog,
		attachPdf: sourceSession.attachPdf,
		requestDetachPdf: sourceSession.requestDetachPdf,
		confirmDetachPdf: sourceSession.confirmDetachPdf,
		browseAndAttachPdf: sourceSession.browseAndAttachPdf,
		handlePdfSearchKeydown: sourceSession.handlePdfSearchKeydown,
		attachFile: sourceSession.attachFile,
		persistableChatHistory: chatSession.persistableChatHistory,
		makeDebugTraceEntry: chatSession.makeDebugTraceEntry,
		renderChatContent: chatSession.renderChatContent,
		setAiInteractionMode: chatSession.setAiInteractionMode,
		handleActiveSectionChange: chatSession.handleActiveSectionChange,
		setToolApproval: chatSession.setToolApproval,
		setStreamingStatus: chatSession.setStreamingStatus,
		visibleAiStatus: chatSession.visibleAiStatus,
		copyMessage: chatSession.copyMessage,
		stopActiveChat: chatSession.stopActiveChat,
		stopChat: chatSession.stopChat,
		beginAiRequest: chatSession.beginAiRequest,
		sendChatMessage: chatSession.sendChatMessage,
		sendChatText: chatSession.sendChatText,
		rewindToSnapshot: chatSession.rewindToSnapshot,
		retryMessage: chatSession.retryMessage,
		mergeChatTools: chatSession.mergeChatTools,
		reconcileRequestNote: chatSession.reconcileRequestNote,
		finishStreamingChatMessage: chatSession.finishStreamingChatMessage,
		extractChatErrorMessage: chatSession.extractChatErrorMessage,
		failStreamingChatMessage: chatSession.failStreamingChatMessage,
		resolveApproval: chatSession.resolveApproval,
		chatContext,
		aiEventContext
	};
}
