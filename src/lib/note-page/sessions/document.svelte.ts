import { invoke } from '@tauri-apps/api/core';
import { tick } from 'svelte';
import { noteOpened } from '$lib/llamaWarm';
import type { NoteDocument } from '$lib/types';

/** Note loading and backend reconciliation lifecycle. */
export function createDocumentSession(ctx: Record<string, any>) {
	async function loadCurrentNote(noteId: string) {
		ctx.isLoadingNote = true;
		ctx.toolsReady = false;
		ctx.clearArmedSelection();
		ctx.writeTargetNotice = false;
		if (ctx.chatPersistTimer) {
			clearTimeout(ctx.chatPersistTimer);
			ctx.chatPersistTimer = undefined;
		}
		const previousAiNoteId = ctx.activeAiNoteId();
		if (previousAiNoteId && previousAiNoteId !== noteId && ctx.chatMessages.length) {
			await ctx.persistChatHistory(previousAiNoteId, ctx.chatMessages);
		}
		ctx.destroyEditorInstance();
		ctx.activeSourceBytes = null;
		ctx.activeSourceId = null;
		ctx.activeSection = null;
		ctx.sectionCache = null;
		ctx.showAttachedNote = false;
		ctx.note = null;
	
		try {
			ctx.note = await invoke<NoteDocument>('load_note', { noteId });
			const loadedNote = ctx.note;
			// Keep the persisted transcript untouched; reasoning is removed only from
			// the assistant's presentation below.
			ctx.chatMessages = loadedNote.chatHistory || [];
			ctx.noteHistory = [];
			ctx.versionPreviewContent = null;
			ctx.activeSidebarTab = 'info';
	
			const relLower = loadedNote.relativePath.toLowerCase();
			ctx.isSourceMaterial =
				relLower.endsWith('.pdf') || relLower.endsWith('.epub') || relLower.endsWith('.html');
	
			if (ctx.isSourceMaterial) {
				ctx.sourceMaterialType = relLower.endsWith('.pdf')
					? 'pdf'
					: relLower.endsWith('.epub')
						? 'epub'
						: 'html';
				ctx.workingDocType = 'md';
	
				const allNotes = await invoke<NoteDocument[]>('get_all_note_documents');
				const existingScratchpad =
					allNotes
						.filter((candidate) => candidate.sourcePdf === loadedNote.id)
						.sort((left, right) => right.updatedAt.localeCompare(left.updatedAt))[0] ?? null;
				ctx.draftTitle = loadedNote.title;
				ctx.draftBody = existingScratchpad?.body ?? '';
				ctx.draftTags = loadedNote.tags.join(', ');
				ctx.activeSourceId = loadedNote.id;
				const bytes = await invoke<ArrayBuffer>('read_pdf_binary', { noteId: loadedNote.id });
				ctx.activeSourceBytes = new Uint8Array(bytes);
				ctx.scratchpadSavedId = existingScratchpad?.id ?? null;
				// Opening the source document should show only that document. The
				// linked note has its own dashboard row and opens in split view from
				// there; it remains available here through the Attach Note button.
				ctx.showAttachedNote = false;
				noteOpened(loadedNote.id, 'chat');
			} else {
				ctx.workingDocType = relLower.endsWith('.tex')
					? 'tex'
					: relLower.endsWith('.ipynb')
						? 'ipynb'
						: 'md';
	
				ctx.draftTitle = loadedNote.title;
				ctx.draftBody = loadedNote.body;
				ctx.draftTags = loadedNote.tags.join(', ');
	
				if (loadedNote.sourcePdf) {
					ctx.activeSourceId = loadedNote.sourcePdf;
					const bytes = await invoke<ArrayBuffer>('read_pdf_binary', {
						noteId: loadedNote.sourcePdf
					});
					ctx.activeSourceBytes = new Uint8Array(bytes);
					// This route was opened through the note itself, so keep its
					// editor visible even when the note is still empty.
					ctx.showAttachedNote = true;
					ctx.scratchpadSavedId = loadedNote.id;
					noteOpened(loadedNote.id, 'chat');
					// If a working document has a sourcePdf, we need to know its type.
					// We'll query it or assume it's PDF for now unless we know otherwise.
					// (We can load it to find out)
					try {
						const sourceDoc = await invoke<NoteDocument>('load_note', {
							noteId: loadedNote.sourcePdf
						});
						const sRel = sourceDoc.relativePath.toLowerCase();
						ctx.sourceMaterialType = sRel.endsWith('.pdf')
							? 'pdf'
							: sRel.endsWith('.epub')
								? 'epub'
								: 'html';
					} catch (e) {
						ctx.sourceMaterialType = 'pdf'; // fallback
					}
				} else {
					ctx.activeSourceId = null;
					ctx.activeSourceBytes = null;
					ctx.activeSection = null;
					ctx.sourceMaterialType = null;
					ctx.showAttachedNote = true;
					ctx.scratchpadSavedId = null;
					noteOpened(loadedNote.id, 'chat');
				}
			}
	
			ctx.message = '';
			void ctx.fetchRelatedNotes();
		} catch (error) {
			console.error('Failed to open note', error);
			ctx.message = 'Could not open this note.';
		} finally {
			ctx.isLoadingNote = false;
			// Let the note and its surrounding layout paint before requesting the
			// comparatively heavy editor/tool bundle.
			if (ctx.note) {
				await tick();
				await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
				ctx.toolsReady = true;
			}
		}
	}
	
	async function refreshCurrentNoteFromBackend(skipEditorUpdate = false) {
		if (!ctx.note) return;
		const refreshed = await invoke<NoteDocument>('load_note', { noteId: ctx.note.id });
		ctx.note = {
			...refreshed,
			chatHistory: ctx.chatMessages
		};
		if (!ctx.isSourceMaterial && ctx.workingDocType === 'md') {
			ctx.draftTitle = refreshed.title;
			ctx.draftBody = refreshed.body;
			ctx.draftTags = refreshed.tags.join(', ');
			if (!skipEditorUpdate && ctx.vditorInstance && ctx.vditorInstance.getValue() !== refreshed.body) {
				ctx.vditorInstance.setValue(refreshed.body);
			}
		} else if (!ctx.isSourceMaterial) {
			ctx.draftTitle = refreshed.title;
			ctx.draftBody = refreshed.body;
			ctx.draftTags = refreshed.tags.join(', ');
		}
		void ctx.fetchRelatedNotes();
	}
	

	return { loadCurrentNote, refreshCurrentNoteFromBackend };
}

