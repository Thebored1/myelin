import { invoke } from '@tauri-apps/api/core';
import { open as openFileDialog } from '@tauri-apps/plugin-dialog';
import type { NoteDocument, PdfAnnotation } from '$lib/types';
import { noteOpened } from '$lib/llamaWarm';

/** Owns source-document, scratchpad, ingestion, and attachment workflows. */
export function createSourceSession(ctx: Record<string, any>) {
	async function attachFile() {
		const picked = await openFileDialog({
			multiple: false,
			filters: [{ name: 'Documents', extensions: ['pdf', 'epub'] }]
		});
		if (typeof picked !== 'string') return;
		try {
			await invoke('import_pdf_file', { filePath: picked, notebook: ctx.openNoteNotebook() });
		} catch (error) {
			console.error('attach failed', error);
		}
	}

	async function handleSectionsReady(sections: any[]) {
		if (!sections.length) return;
		const aiNoteId = ctx.activeAiNoteId();
		if (!aiNoteId) return;
		ctx.sectionCache = {
			done: 0, total: sections.length, sectionDone: 0, sectionTotal: sections.length,
			label: sections[0]?.label ?? '', profile: '', startedAt: performance.now(), finished: false,
			failed: 0, failedDetails: [], elapsedMs: null
		};
		try {
			await invoke('cache_note_sections', {
				noteId: aiNoteId,
				sections,
				activeSectionKey: ctx.activeSection?.key ?? sections[0]?.key ?? null,
				interactionMode: null
			});
		} catch (error) {
			if (ctx.sectionCache && !ctx.sectionCache.finished) {
				ctx.sectionCache = {
					...ctx.sectionCache,
					failed: Math.max(ctx.sectionCache.failed, ctx.sectionCache.total - ctx.sectionCache.done),
					finished: true,
					elapsedMs: Math.max(0, performance.now() - ctx.sectionCache.startedAt)
				};
			}
			console.debug('Section pre-cache skipped:', error);
		}
	}

	function formatSectionCacheDuration(milliseconds: number): string {
		if (milliseconds < 1000) return `${Math.round(milliseconds)} ms`;
		const seconds = milliseconds / 1000;
		if (seconds < 60) return `${seconds.toFixed(seconds < 10 ? 1 : 0)} s`;
		const minutes = Math.floor(seconds / 60);
		return `${minutes}m ${Math.round(seconds % 60)}s`;
	}

	async function openAttachedNote() {
		if (!ctx.note || !ctx.isSourceMaterial) return;
		if (!ctx.scratchpadSavedId) {
			const created = await invoke<NoteDocument>('create_note', {
				title: ctx.draftTitle,
				sourcePdf: ctx.activeSourceId,
				notebook: ctx.openNoteNotebook()
			});
			ctx.scratchpadSavedId = created.id;
			ctx.draftTitle = created.title;
			ctx.draftBody = created.body;
			ctx.draftTags = created.tags.join(', ');
			ctx.chatMessages = created.chatHistory || [];
		} else {
			const workingNote = await invoke<NoteDocument>('load_note', { noteId: ctx.scratchpadSavedId });
			ctx.draftTitle = workingNote.title;
			ctx.draftBody = workingNote.body;
			ctx.draftTags = workingNote.tags.join(', ');
			ctx.chatMessages = workingNote.chatHistory || [];
		}
		ctx.showAttachedNote = true;
		noteOpened(ctx.scratchpadSavedId, 'chat');
		await ctx.tick();
		setTimeout(() => ctx.initVditor(), 100);
	}

	function handlePdfQuote(text: string, page: number) {
		ctx.appendToNoteBody(`\n> ${text}\n> *(Page ${page})*\n\n`);
	}

	function handleAnnotationsChange(annotations: PdfAnnotation[]) {
		if (ctx.note) {
			ctx.note.annotations = annotations;
			ctx.triggerAutoSave();
		}
	}

	function handleImageExtract(base64: string) {
		ctx.appendToNoteBody(`\n\n![Extracted Image](${base64})\n\n`);
	}

	function handlePdfTextExtracted(text: string) {
		if (!ctx.activeSourceId || !ctx.note) return;
		const sourceId = ctx.activeSourceId;
		const sourceTitle = ctx.isSourceMaterial ? ctx.note.title : `${ctx.draftTitle} — attached PDF`;
		ctx.pdfIngestionStatus = 'indexing';
		ctx.pdfIngestionError = null;
		const startedAt = Date.now();
		const startEntry = {
			time: startedAt,
			kind: 'config',
			msg: `PDF indexing started: ${sourceTitle} (${sourceId}), ${text.length.toLocaleString()} extracted characters`
		};
		ctx.pendingDebugTrace = [...ctx.pendingDebugTrace, startEntry];
		ctx.pdfIngestionPromise = (async () => {
			try {
				const result = await invoke<{ status: 'cached' | 'indexed' | 'empty'; chunks: number }>(
					'ensure_document_ingested', { docId: sourceId, source: sourceTitle, text }
				);
				const entry = { time: Date.now(), kind: 'done', msg: `PDF indexing ${result.status}: ${sourceTitle} (${result.chunks} chunks)` };
				ctx.pendingDebugTrace = [...ctx.pendingDebugTrace, entry];
				if (ctx.debugInfo) ctx.debugInfo = { ...ctx.debugInfo, trace: [...ctx.debugInfo.trace, startEntry, entry] };
				if (ctx.activeSourceId === sourceId) ctx.pdfIngestionStatus = result.status;
			} catch (error) {
				console.error('Failed to index PDF text', error);
				const detail = typeof error === 'string' ? error : error instanceof Error ? error.message : JSON.stringify(error) || String(error);
				const entry = { time: Date.now(), kind: 'error', msg: `PDF indexing failed for ${sourceTitle} (${sourceId}): ${detail}` };
				ctx.pendingDebugTrace = [...ctx.pendingDebugTrace, entry];
				ctx.showDebugWindow = true;
				ctx.debugInfo = ctx.debugInfo ? { ...ctx.debugInfo, trace: [...ctx.debugInfo.trace, startEntry, entry] } : {
					requestStart: startedAt, firstChunk: null, generationStart: null, generationEnd: null,
					done: entry.time, promptTokens: 0, completionTokens: 0, totalTokens: 0, turnCount: 0,
					replyChars: 0, trace: [startEntry, entry]
				};
				if (ctx.activeSourceId === sourceId) {
					ctx.pdfIngestionStatus = 'failed';
					ctx.pdfIngestionError = detail;
				}
			}
		})();
	}

	async function openAttachPdfDialog() {
		ctx.pdfSearchQuery = '';
		ctx.pdfSelectedIndex = 0;
		ctx.isBusy = true;
		try {
			const allDocs = await invoke<NoteDocument[]>('get_all_note_documents');
			const referenced = new Set(allDocs.map((d) => d.sourcePdf).filter((id): id is string => !!id));
			const isCopyName = (d: NoteDocument) => {
				const name = d.relativePath.split(/[\\/]/).pop()?.toLowerCase() ?? '';
				return / \d+\.(pdf|epub)$/.test(name) || / [0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\.(pdf|epub)$/.test(name);
			};
			ctx.pdfNotesList = allDocs.filter((d) => d.relativePath.toLowerCase().endsWith('.pdf') && !(referenced.has(d.id) && isCopyName(d)));
		} catch (err) {
			ctx.message = `Failed to load PDFs: ${err}`;
		} finally {
			ctx.isBusy = false;
		}
		ctx.attachPdfDialog?.showModal();
		setTimeout(() => (ctx.attachPdfDialog?.querySelector('.link-search-input') as HTMLInputElement | null)?.focus(), 50);
	}

	async function attachPdf(pdfNote: NoteDocument, alreadyImported = false) {
		if (!ctx.note) return;
		ctx.attachPdfDialog?.close();
		ctx.isBusy = true;
		let createdPdfId: string | null = alreadyImported ? pdfNote.id : null;
		try {
			const attachmentPdf = alreadyImported ? pdfNote : await invoke<NoteDocument>('clone_pdf_for_attachment', {
				noteId: pdfNote.id, notebook: ctx.openNoteNotebook()
			});
			createdPdfId = attachmentPdf.id;
			ctx.note = await invoke<NoteDocument>('save_note', {
				noteId: ctx.note.id, title: ctx.draftTitle,
				tags: ctx.draftTags.split(',').map((t: string) => t.trim()).filter(Boolean),
				body: ctx.draftBody, sourcePdf: attachmentPdf.id, annotations: ctx.note.annotations
			});
			ctx.activeSourceId = attachmentPdf.id;
			ctx.sectionCache = null;
			const bytes = await invoke<ArrayBuffer>('read_pdf_binary', { noteId: attachmentPdf.id });
			ctx.activeSourceBytes = new Uint8Array(bytes);
			ctx.sourceMaterialType = 'pdf';
			ctx.showAttachedNote = true;
			ctx.saveStatus = 'saved';
			ctx.destroyEditorInstance();
			await ctx.tick();
			ctx.initVditor();
		} catch (err) {
			if (createdPdfId) {
				try { await invoke('delete_note', { noteId: createdPdfId }); }
				catch (cleanupError) { console.warn('Failed to clean up copied PDF after attachment failure', cleanupError); }
			}
			ctx.message = `Failed to attach PDF: ${err}`;
		} finally {
			ctx.isBusy = false;
		}
	}

	function requestDetachPdf() { ctx.detachPdfDialog?.showModal(); }

	async function confirmDetachPdf() {
		ctx.detachPdfDialog?.close();
		if (!ctx.note) return;
		ctx.isBusy = true;
		try {
			ctx.note = await invoke<NoteDocument>('save_note', {
				noteId: ctx.note.id, title: ctx.draftTitle,
				tags: ctx.draftTags.split(',').map((t: string) => t.trim()).filter(Boolean),
				body: ctx.draftBody, sourcePdf: null, annotations: ctx.note.annotations
			});
			ctx.activeSourceId = null;
			ctx.activeSourceBytes = null;
			ctx.activeSection = null;
			ctx.sectionCache = null;
			ctx.saveStatus = 'saved';
			ctx.destroyEditorInstance();
			await ctx.tick();
			ctx.initVditor();
		} catch (err) {
			ctx.message = `Failed to detach PDF: ${err}`;
		} finally {
			ctx.isBusy = false;
		}
	}

	async function browseAndAttachPdf() {
		const selected = await openFileDialog({ multiple: false, filters: [{ name: 'Documents', extensions: ['pdf', 'epub', 'tex', 'ipynb', 'md'] }] });
		if (!selected) return;
		ctx.attachPdfDialog?.close();
		ctx.isBusy = true;
		try {
			const pdfNote = await invoke<NoteDocument>('import_pdf_file', { filePath: selected, notebook: ctx.openNoteNotebook() });
			await attachPdf(pdfNote, true);
		} catch (err) {
			ctx.message = `Failed to import PDF: ${err}`;
			ctx.isBusy = false;
		}
	}

	function handlePdfSearchKeydown(e: KeyboardEvent) {
		if (e.key === 'ArrowDown') { e.preventDefault(); ctx.pdfSelectedIndex = Math.min(ctx.filteredPdfs.length - 1, ctx.pdfSelectedIndex + 1); }
		else if (e.key === 'ArrowUp') { e.preventDefault(); ctx.pdfSelectedIndex = Math.max(0, ctx.pdfSelectedIndex - 1); }
		else if (e.key === 'Enter') { e.preventDefault(); if (ctx.filteredPdfs.length > 0) attachPdf(ctx.filteredPdfs[ctx.pdfSelectedIndex]); }
	}

	function dispose() { ctx.pdfIngestionPromise = null; }

	return {
		attachFile,
		handleSectionsReady, formatSectionCacheDuration, openAttachedNote, handlePdfQuote,
		handleAnnotationsChange, handleImageExtract, handlePdfTextExtracted,
		openAttachPdfDialog, attachPdf, requestDetachPdf, confirmDetachPdf, browseAndAttachPdf,
		handlePdfSearchKeydown, dispose
	};
}
