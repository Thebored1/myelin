import type { ControllerContext } from '$lib/controller-context';

/** Owns note-body insertion, editor destruction, and debounced persistence state. */
export function createEditorStateSession(rawContext: object) {
	const ctx = rawContext as ControllerContext;
	function appendToNoteBody(content: string) {
		ctx.showAttachedNote = true;
		if (ctx.vditorInstance) {
			ctx.vditorInstance.insertValue(content);
			ctx.draftBody = ctx.vditorInstance.getValue();
		} else {
			ctx.draftBody = `${ctx.draftBody}${content}`;
		}
		ctx.triggerAutoSave();
	}

	function destroyEditorInstance() {
		ctx.invalidateVditorInitialization?.();
		if (!ctx.vditorInstance) return;
		try {
			ctx.vditorInstance.destroy();
		} catch (error) {
			console.warn('Vditor destroy error:', error);
		}
		ctx.vditorInstance = null;
	}

	function triggerAutoSave() {
		if (ctx.saveStatus !== 'saving') ctx.saveStatus = 'unsaved';
		if (ctx.saveTimer) clearTimeout(ctx.saveTimer);
		ctx.saveTimer = setTimeout(() => void ctx.saveNote(), 1000);
	}

	return { appendToNoteBody, destroyEditorInstance, triggerAutoSave };
}
