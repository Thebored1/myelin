/** Owns modal-only workflows that coordinate document and navigation sessions. */
import type { ControllerContext } from '$lib/controller-context';

export function createNotePageDialogs(rawContext: object) {
	const ctx = rawContext as ControllerContext;
	function requestDeleteMainNote() {
		ctx.deleteMainNoteDialog?.showModal();
	}

	function requestDeleteAttachedNote() {
		ctx.deleteAttachedNoteDialog?.showModal();
	}

	async function confirmDeleteAttachedNote() {
		ctx.deleteAttachedNoteDialog?.close();
		const targetId = ctx.isSourceMaterial
			? ctx.scratchpadSavedId
			: ctx.note?.sourcePdf
				? ctx.note.id
				: null;
		const sourceId = ctx.isSourceMaterial
			? ctx.activeSourceId
			: (ctx.note?.sourcePdf ?? ctx.activeSourceId);
		ctx.isBusy = true;
		try {
			if (targetId) await ctx.deleteNote(targetId);
			if (!ctx.isSourceMaterial && sourceId) {
				ctx.navigationSession.isProgrammaticNavigation = true;
				await ctx.goToNote(sourceId);
				return;
			}
			if (ctx.saveTimer) {
				clearTimeout(ctx.saveTimer);
				ctx.saveTimer = null;
			}
			ctx.destroyEditorInstance();
			ctx.draftBody = '';
			ctx.scratchpadSavedId = null;
			ctx.showAttachedNote = false;
			ctx.saveStatus = 'saved';
			ctx.message = '';
		} finally {
			ctx.isBusy = false;
		}
	}

	function cancelDeleteAttachedNote() {
		ctx.deleteAttachedNoteDialog?.close();
	}

	function buildPreviewExpandHref() {
		const targetId =
			ctx.linkingSession.previewNoteTarget?.sourcePdf ?? ctx.linkingSession.previewNoteTarget?.id;
		const currentNoteId = ctx.note?.id;
		if (!targetId) return null;
		const basePath = `/notes/${encodeURIComponent(targetId)}`;
		if (!currentNoteId) return basePath;
		return `${basePath}?returnTo=/notes/${encodeURIComponent(currentNoteId)}`;
	}

	function expandPreviewNoteDirect() {
		const href = buildPreviewExpandHref();
		if (!href) return;
		ctx.linkingSession.previewNoteDialog?.close();
		ctx.navigationSession.isProgrammaticNavigation = true;
		ctx.goToHref(href);
	}

	return {
		requestDeleteMainNote,
		requestDeleteAttachedNote,
		confirmDeleteAttachedNote,
		cancelDeleteAttachedNote,
		buildPreviewExpandHref,
		expandPreviewNoteDirect
	};
}
