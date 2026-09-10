import { createLatexSession } from './latex.svelte';
import { createSelectionSession } from './selection.svelte';
import { createMathSession } from './math.svelte';
import { createLayoutSession } from './layout.svelte';
import type { ControllerContext } from '$lib/controller-context';

/** Builds the editor-adjacent sessions that share selection and layout state. */
export function createNoteInputGraph(rawContext: object) {
	const ctx = rawContext as ControllerContext;
	const latexSession = createLatexSession({
		get note() {
			return ctx.note;
		},
		get texCompileError() {
			return ctx.texCompileError;
		},
		set texCompileError(value) {
			ctx.texCompileError = value;
		},
		get texPreviewStatus() {
			return ctx.texPreviewStatus;
		},
		set texPreviewStatus(value) {
			ctx.texPreviewStatus = value;
		},
		get texCacheWarmed() {
			return ctx.texCacheWarmed;
		},
		set texCacheWarmed(value) {
			ctx.texCacheWarmed = value;
		},
		get texCompiling() {
			return ctx.texCompiling;
		},
		set texCompiling(value) {
			ctx.texCompiling = value;
		},
		get texCompileQueued() {
			return ctx.texCompileQueued;
		},
		set texCompileQueued(value) {
			ctx.texCompileQueued = value;
		},
		get isBusy() {
			return ctx.isBusy;
		},
		set isBusy(value) {
			ctx.isBusy = value;
		},
		get texRevision() {
			return ctx.texRevision;
		},
		set texRevision(value) {
			ctx.texRevision = value;
		},
		get draftBody() {
			return ctx.draftBody;
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
		get showAttachedNote() {
			return ctx.showAttachedNote;
		},
		set showAttachedNote(value) {
			ctx.showAttachedNote = value;
		},
		get texDiagnostics() {
			return ctx.texDiagnostics;
		},
		set texDiagnostics(value) {
			ctx.texDiagnostics = value;
		},
		get latexDownloadMsg() {
			return ctx.latexDownloadMsg;
		},
		set latexDownloadMsg(value) {
			ctx.latexDownloadMsg = value;
		},
		get activeSection() {
			return ctx.activeSection;
		},
		set activeSection(value) {
			ctx.activeSection = value;
		},
		get sectionCache() {
			return ctx.sectionCache;
		},
		set sectionCache(value) {
			ctx.sectionCache = value;
		},
		get workingDocType() {
			return ctx.workingDocType;
		},
		get lastTexBody() {
			return ctx.lastTexBody;
		},
		set lastTexBody(value) {
			ctx.lastTexBody = value;
		},
		get texAutoCompile() {
			return ctx.texAutoCompile;
		},
		get texAutoTimer() {
			return ctx.texAutoTimer;
		},
		set texAutoTimer(value) {
			ctx.texAutoTimer = value;
		},
		saveNote: ctx.saveNote
	});

	const selectionSession = createSelectionSession({
		get vditorInstance() {
			return ctx.vditorInstance;
		},
		get vditorContainer() {
			return ctx.vditorContainer;
		},
		get armedSelection() {
			return ctx.armedSelection;
		},
		set armedSelection(value) {
			ctx.armedSelection = value;
		},
		get writeTargetNotice() {
			return ctx.writeTargetNotice;
		},
		set writeTargetNotice(value) {
			ctx.writeTargetNotice = value;
		},
		get selDebounce() {
			return ctx.selDebounce;
		},
		set selDebounce(value) {
			ctx.selDebounce = value;
		},
		get savedEditorRange() {
			return ctx.savedEditorRange;
		},
		set savedEditorRange(value) {
			ctx.savedEditorRange = value;
		},
		get draftBody() {
			return ctx.draftBody;
		},
		set draftBody(value) {
			ctx.draftBody = value;
		},
		get noteStreaming() {
			return ctx.noteStreaming;
		},
		triggerAutoSave: ctx.triggerAutoSave,
		focusEditor: ctx.focusEditor
	});
	ctx.setSelectionSession(selectionSession);

	const mathSession = createMathSession({
		get vditorInstance() {
			return ctx.vditorInstance;
		}
	});
	const layoutSession = createLayoutSession({
		PANE_MIN_WIDTH: ctx.PANE_MIN_WIDTH,
		SIDEBAR_MIN_WIDTH: ctx.SIDEBAR_MIN_WIDTH,
		get splitRatio() {
			return ctx.splitRatio;
		},
		set splitRatio(value) {
			ctx.splitRatio = value;
		},
		get isResizing() {
			return ctx.isResizing;
		},
		set isResizing(value) {
			ctx.isResizing = value;
		},
		get mainLayoutEl() {
			return ctx.mainLayoutEl;
		},
		get sidebarWidth() {
			return ctx.sidebarWidth;
		},
		set sidebarWidth(value) {
			ctx.sidebarWidth = value;
		},
		get isSidebarResizing() {
			return ctx.isSidebarResizing;
		},
		set isSidebarResizing(value) {
			ctx.isSidebarResizing = value;
		},
		get vditorInstance() {
			return ctx.vditorInstance;
		},
		get note() {
			return ctx.note;
		},
		get activeSidebarTab() {
			return ctx.activeSidebarTab;
		},
		set activeSidebarTab(value) {
			ctx.activeSidebarTab = value;
		},
		get chatTextareaEl() {
			return ctx.chatTextareaEl;
		},
		get chatMessagesEl() {
			return ctx.chatMessagesEl;
		},
		get chatMessages() {
			return ctx.chatMessages;
		},
		get userScrolledUp() {
			return ctx.userScrolledUp;
		},
		set userScrolledUp(value) {
			ctx.userScrolledUp = value;
		},
		captureShortcutEditorTarget: ctx.captureShortcutEditorTarget,
		restoreShortcutEditorFocus: ctx.restoreShortcutEditorFocus,
		tick: ctx.tick
	});

	return {
		latexSession,
		pickLatexImage: latexSession.pickLatexImage,
		compileTex: latexSession.compileTex,
		parseLatexError: latexSession.parseLatexError,
		closeTexPreview: latexSession.closeTexPreview,
		selectionSession,
		getSelectionTextOffset: selectionSession.getSelectionTextOffset,
		textOffsetOf: selectionSession.textOffsetOf,
		nearestIndexOf: selectionSession.nearestIndexOf,
		computeSourceSelection: selectionSession.computeSourceSelection,
		computeSourceCursor: selectionSession.computeSourceCursor,
		clearArmedSelection: selectionSession.clearArmedSelection,
		onDocMouseDown: selectionSession.onDocMouseDown,
		captureEditorSelection: selectionSession.captureEditorSelection,
		captureExternalTarget: selectionSession.captureExternalTarget,
		reselectAfterEdit: selectionSession.reselectAfterEdit,
		armedEditTarget: selectionSession.armedEditTarget,
		onSelectionChange: selectionSession.onSelectionChange,
		handleGlobalSelectionChange: selectionSession.handleGlobalSelectionChange,
		restoreSelectionTextOffset: selectionSession.restoreSelectionTextOffset,
		saveCursorPosition: selectionSession.saveCursorPosition,
		insertAtSavedCursor: selectionSession.insertAtSavedCursor,
		mathSession,
		openMathDialog: mathSession.openMathDialog,
		insertMath: mathSession.insertMath,
		layoutSession,
		startSidebarResizing: layoutSession.startSidebarResizing,
		startResizing: layoutSession.startResizing,
		handleGlobalMouseMove: layoutSession.handleGlobalMouseMove,
		stopResizing: layoutSession.stopResizing,
		handleChatSidebarShortcut: layoutSession.handleChatSidebarShortcut,
		handleChatScroll: layoutSession.handleChatScroll,
		scrollChatToBottom: layoutSession.scrollChatToBottom
	};
}
