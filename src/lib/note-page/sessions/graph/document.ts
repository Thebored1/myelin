import type { ControllerContext } from '$lib/controller-context';
import type { createDocumentSession as createDocumentSessionImpl } from '../document.svelte';
import type { createNavigationSession } from '../navigation.svelte';
import type { createEditorSession } from '../editor.svelte';

type GraphContext = ControllerContext;
type DocumentSession = ReturnType<typeof createDocumentSessionImpl>;

export function createDocumentSession(
	ctx: GraphContext,
	persistChatHistory: (...args: never[]) => unknown,
	getNavigationSession: () => ReturnType<typeof createNavigationSession>,
	getEditorSession: () => ReturnType<typeof createEditorSession>
): DocumentSession {
	const documentSession = (ctx.createDocumentSession as (context: object) => unknown)({
		get isLoadingNote() {
			return ctx.isLoadingNote;
		},
		set isLoadingNote(value) {
			ctx.isLoadingNote = value;
		},
		get toolsReady() {
			return ctx.toolsReady;
		},
		set toolsReady(value) {
			ctx.toolsReady = value;
		},
		clearArmedSelection: ctx.clearArmedSelection,
		get writeTargetNotice() {
			return ctx.writeTargetNotice;
		},
		set writeTargetNotice(value) {
			ctx.writeTargetNotice = value;
		},
		get chatPersistTimer() {
			return ctx.chatPersistTimer;
		},
		set chatPersistTimer(value) {
			ctx.chatPersistTimer = value;
		},
		activeAiNoteId: ctx.activeAiNoteId,
		persistChatHistory,
		destroyEditorInstance: ctx.destroyEditorInstance,
		get activeSourceBytes() {
			return ctx.activeSourceBytes;
		},
		set activeSourceBytes(value) {
			ctx.activeSourceBytes = value;
		},
		get activeSourceId() {
			return ctx.activeSourceId;
		},
		set activeSourceId(value) {
			ctx.activeSourceId = value;
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
		get showAttachedNote() {
			return ctx.showAttachedNote;
		},
		set showAttachedNote(value) {
			ctx.showAttachedNote = value;
		},
		get note() {
			return ctx.note;
		},
		set note(value) {
			ctx.note = value;
		},
		get chatMessages() {
			return ctx.chatMessages;
		},
		set chatMessages(value) {
			ctx.chatMessages = value;
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
		get activeSidebarTab() {
			return ctx.activeSidebarTab;
		},
		set activeSidebarTab(value) {
			ctx.activeSidebarTab = value;
		},
		get isSourceMaterial() {
			return ctx.isSourceMaterial;
		},
		set isSourceMaterial(value) {
			ctx.isSourceMaterial = value;
		},
		get sourceMaterialType() {
			return ctx.sourceMaterialType;
		},
		set sourceMaterialType(value) {
			ctx.sourceMaterialType = value;
		},
		get workingDocType() {
			return ctx.workingDocType;
		},
		set workingDocType(value) {
			ctx.workingDocType = value;
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
		get scratchpadSavedId() {
			return ctx.scratchpadSavedId;
		},
		set scratchpadSavedId(value) {
			ctx.scratchpadSavedId = value;
		},
		get message() {
			return ctx.message;
		},
		set message(value) {
			ctx.message = value;
		},
		openNoteNotebook: ctx.openNoteNotebook,
		get saveStatus() {
			return ctx.saveStatus;
		},
		set saveStatus(value) {
			ctx.saveStatus = value;
		},
		fetchNoteHistory: () => getNavigationSession().fetchNoteHistory(),
		fetchRelatedNotes: () => getEditorSession().fetchRelatedNotes(),
		get vditorInstance() {
			return ctx.vditorInstance;
		},
		get isBusy() {
			return ctx.isBusy;
		},
		set isBusy(value) {
			ctx.isBusy = value;
		},
		goToHome: ctx.goToHome,
		safeNavigate: (url: string) => getNavigationSession().safeNavigate(url)
	});
	return documentSession as DocumentSession;
}
