import type { ControllerContext } from '$lib/controller-context';

/** Builds the command surface exposed by the note-page controller. */
export function createNotePageActions(rawContext: object) {
	const ctx = rawContext as ControllerContext;
	return {
		appendToNoteBody: ctx.appendToNoteBody,
		destroyEditorInstance: ctx.destroyEditorInstance,
		triggerAutoSave: ctx.triggerAutoSave,
		pickLatexImage: ctx.pickLatexImage,
		compileTex: ctx.compileTex,
		parseLatexError: ctx.parseLatexError,
		closeTexPreview: ctx.closeTexPreview,
		parseBlocks: ctx.editorSession.parseBlocks,
		...ctx.linkingSession,
		loadCurrentNote: ctx.loadCurrentNote,
		refreshCurrentNoteFromBackend: ctx.refreshCurrentNoteFromBackend,
		beginNoteStream: ctx.beginNoteStream,
		scheduleNoteStreamFlush: ctx.scheduleNoteStreamFlush,
		flushNoteStream: ctx.flushNoteStream,
		appendNoteStream: ctx.appendNoteStream,
		cancelNoteStream: ctx.cancelNoteStream,
		applyNoteWrite: ctx.applyNoteWrite,
		initVditor: ctx.initVditor,
		scanForTransclusions: ctx.scanForTransclusions,
		setupTransclusionObserver: ctx.setupTransclusionObserver,
		fetchRelatedNotes: ctx.fetchRelatedNotes,
		handleAnnotationsChange: ctx.handleAnnotationsChange,
		handleImageExtract: ctx.handleImageExtract,
		handlePdfTextExtracted: ctx.handlePdfTextExtracted,
		saveNote: ctx.saveNote,
		deleteNote: ctx.deleteNote,
		duplicateNote: ctx.duplicateNote,
		stopActiveChat: ctx.stopActiveChat,
		stopChat: ctx.stopChat,
		beginAiRequest: ctx.beginAiRequest,
		sendChatMessage: ctx.sendChatMessage,
		sendChatText: ctx.sendChatText,
		rewindToSnapshot: ctx.rewindToSnapshot,
		retryMessage: ctx.retryMessage,
		mergeChatTools: ctx.mergeChatTools,
		reconcileRequestNote: ctx.reconcileRequestNote,
		finishStreamingChatMessage: ctx.finishStreamingChatMessage,
		extractChatErrorMessage: ctx.extractChatErrorMessage,
		failStreamingChatMessage: ctx.failStreamingChatMessage,
		resolveApproval: ctx.resolveApproval,
		fetchNoteHistory: ctx.fetchNoteHistory,
		previewVersion: ctx.previewVersion,
		restoreVersion: ctx.restoreVersion,
		get pendingNavigationUrl() {
			return ctx.navigationSession.pendingNavigationUrl;
		},
		set pendingNavigationUrl(value) {
			ctx.navigationSession.pendingNavigationUrl = value;
		},
		get pendingBack() {
			return ctx.navigationSession.pendingBack;
		},
		set pendingBack(value) {
			ctx.navigationSession.pendingBack = value;
		},
		get isProgrammaticNavigation() {
			return ctx.navigationSession.isProgrammaticNavigation;
		},
		set isProgrammaticNavigation(value) {
			ctx.navigationSession.isProgrammaticNavigation = value;
		},
		safeNavigate: ctx.safeNavigate,
		goBack: ctx.goBack,
		navigateBack: ctx.navigateBack,
		requestDeleteAttachedNote: ctx.requestDeleteAttachedNote,
		confirmDeleteAttachedNote: ctx.confirmDeleteAttachedNote,
		cancelDeleteAttachedNote: ctx.cancelDeleteAttachedNote,
		openAttachPdfDialog: ctx.openAttachPdfDialog,
		attachPdf: ctx.attachPdf,
		requestDetachPdf: ctx.requestDetachPdf,
		confirmDetachPdf: ctx.confirmDetachPdf,
		browseAndAttachPdf: ctx.browseAndAttachPdf,
		handlePdfSearchKeydown: ctx.handlePdfSearchKeydown,
		buildPreviewExpandHref: ctx.buildPreviewExpandHref,
		expandPreviewNoteDirect: ctx.expandPreviewNoteDirect,
		handleBeforeUnload: ctx.handleBeforeUnload,
		confirmNavigation: ctx.confirmNavigation,
		cancelNavigation: ctx.cancelNavigation,
		updateToolbarOverflow: ctx.updateToolbarOverflow,
		handleGlobalSelectionChange: ctx.handleGlobalSelectionChange
	};
}
