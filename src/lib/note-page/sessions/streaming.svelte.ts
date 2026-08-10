import { composeNoteStreamPreviewWithStatus, locateNoteStreamTarget } from '$lib/noteStreamPreview';
import { editorNeedsAuthoritativeBody } from '$lib/noteMutation';

/** Buffered AI note-write preview and authoritative reconciliation session. */
export function createStreamingSession(ctx: Record<string, any>) {
	function beginNoteStream() {
		ctx.noteStreamBackup = ctx.vditorInstance ? ctx.vditorInstance.getValue() : ctx.draftBody;
		ctx.noteStreamBuf = '';
		ctx.noteStreaming = true;
		// Every stream flush rebuilds the whole IR DOM; keep the transclusion
		// observer disconnected until the stream settles so it doesn't drain
		// full-tree mutation batches each frame. scanForTransclusions runs once
		// after the stream lands (setupTransclusionObserver re-arms it).
		if (ctx.transclusionObserver) ctx.transclusionObserver.disconnect();
		// Locate the cursor/selection target once: it is stable for the whole
		// request, so per-delta re-scanning a large note is wasted work.
		ctx.noteStreamSpan = ctx.activeAiEditTarget
			? locateNoteStreamTarget(ctx.noteStreamBackup, ctx.activeAiEditTarget)
			: null;
		scheduleNoteStreamFlush();
	}
	
	function scheduleNoteStreamFlush() {
		if (ctx.noteStreamFlushPending) return;
		ctx.noteStreamFlushPending = true;
		requestAnimationFrame(() => {
			ctx.noteStreamFlushPending = false;
			flushNoteStream();
		});
	}
	
	// One editor rebuild per frame, coalescing all deltas that arrived since the
	// last flush. Restores the caret/selection across the rebuild so streaming
	// no longer destroys the user's cursor position every token.
	function flushNoteStream() {
		if (!ctx.noteStreaming) return;
		const result = composeNoteStreamPreviewWithStatus(
			ctx.noteStreamBackup,
			ctx.noteStreamBuf,
			ctx.activeAiEditTarget,
			ctx.noteStreamSpan
		);
		if (ctx.vditorInstance) {
			// getSelectionTextOffset walks every text node of the editor; only do
			// it when the editor actually has focus and a live selection. The
			// user isn't interacting mid-stream, so skip the walk otherwise.
			const editorEl = ctx.vditorContainer?.querySelector('.vditor-ir') as HTMLElement | null;
			const hasSelection =
				editorEl?.contains(document.activeElement) && window.getSelection()?.rangeCount !== 0;
			const selectionOffset = editorEl && hasSelection ? ctx.getSelectionTextOffset(editorEl) : null;
			ctx.vditorInstance.setValue(result.preview);
			if (selectionOffset !== null) {
				const refreshed = ctx.vditorContainer?.querySelector('.vditor-ir') as HTMLElement | null;
				if (refreshed) {
					ctx.restoreSelectionTextOffset(refreshed, Math.min(selectionOffset, result.preview.length));
				}
			}
			// Keep draftBody in sync with the live preview so any mid-stream save
			// (title/tag autosave, exit) carries the streamed content instead of
			// racing the backend with a stale body.
			ctx.draftBody = result.preview;
		}
	}
	
	// A token (or several) of the note arrived — buffer it and coalesce the
	// editor update to the next animation frame.
	function appendNoteStream(delta: string): boolean {
		if (!ctx.noteStreaming) beginNoteStream();
		ctx.noteStreamBuf += delta;
		scheduleNoteStreamFlush();
		return ctx.noteStreamSpan !== null || !ctx.activeAiEditTarget;
	}
	
	// The stream turned out not to be a whole-body replace (append/edit) — undo
	// the live preview; the authoritative note_written will apply the real change.
	function cancelNoteStream() {
		if (!ctx.noteStreaming) return;
		ctx.noteStreaming = false;
		if (ctx.vditorInstance) ctx.vditorInstance.setValue(ctx.noteStreamBackup);
		ctx.setupTransclusionObserver();
	}
	
	// Authoritative result of a write_note tool call. Sets the final content in
	// one shot (no fake animation) and reconciles any live-streamed preview.
	function applyNoteWrite(newContent: string, mode: 'write' | 'append') {
		ctx.noteStreaming = false;
		// Rust emits the full authoritative body for every note mutation. Keep a
		// compatibility path for older sidecars that may still send an append
		// fragment, but never duplicate a full body that already contains the
		// current note prefix.
		const currentContent = ctx.vditorInstance ? ctx.vditorInstance.getValue() : ctx.draftBody;
		const currentTrimmed = currentContent.trimEnd();
		const isAuthoritativeBody =
			mode === 'write' ||
			newContent === currentContent ||
			(mode === 'append' && currentTrimmed.length > 0 && newContent.startsWith(currentTrimmed));
		const finalContent = isAuthoritativeBody
			? newContent
			: currentTrimmed
				? `${currentTrimmed}\n\n${newContent}`
				: newContent;
		if (ctx.note) ctx.note = { ...ctx.note, body: finalContent };
		ctx.draftBody = finalContent;
		// Avoid a second visible reset only when the editor itself already contains
		// the authoritative result. The streamed buffer may be stale or may cover
		// only a cursor/selection target. clearStack resets Vditor's undo history so
		// Ctrl+Z doesn't walk back through every mid-stream snapshot.
		if (ctx.vditorInstance && ctx.editorNeedsAuthoritativeBody(ctx.vditorInstance.getValue(), finalContent)) {
			ctx.vditorInstance.setValue(finalContent, true);
		}
		// Re-arm the transclusion observer disconnected during streaming and scan
		// once so the settled content picks up any new links.
		ctx.setupTransclusionObserver();
	}
	

	return { beginNoteStream, scheduleNoteStreamFlush, flushNoteStream, appendNoteStream, cancelNoteStream, applyNoteWrite };
}
