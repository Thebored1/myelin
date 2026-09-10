import type { ControllerContext } from '$lib/controller-context';
import type { createLinkingSession as createLinkingSessionImpl } from '../linking.svelte';

type GraphContext = ControllerContext;
type LinkingSession = ReturnType<typeof createLinkingSessionImpl>;

export function createLinkingSession(ctx: GraphContext): LinkingSession {
	const linkingSession = (ctx.createLinkingSession as (context: object) => unknown)({
		get isBusy() {
			return ctx.isBusy;
		},
		set isBusy(value) {
			ctx.isBusy = value;
		},
		get VditorConstructor() {
			return ctx.VditorConstructor;
		},
		localVditorCdn: ctx.localVditorCdn,
		get shouldRefocusEditor() {
			return ctx.shouldRefocusEditor;
		},
		set shouldRefocusEditor(value) {
			ctx.shouldRefocusEditor = value;
		},
		insertAtSavedCursor: ctx.insertAtSavedCursor,
		refocusEditorSoon: ctx.refocusEditorSoon,
		get vditorInstance() {
			return ctx.vditorInstance;
		},
		get vditorContainer() {
			return ctx.vditorContainer;
		},
		getSelectionTextOffset: ctx.getSelectionTextOffset,
		get draftBody() {
			return ctx.draftBody;
		},
		set draftBody(value) {
			ctx.draftBody = value;
		},
		get note() {
			return ctx.note;
		},
		get message() {
			return ctx.message;
		},
		set message(value) {
			ctx.message = value;
		},
		saveCursorPosition: ctx.saveCursorPosition,
		focusEditor: ctx.focusEditor,
		restoreSelectionTextOffset: ctx.restoreSelectionTextOffset
	});

	return linkingSession as LinkingSession;
}
