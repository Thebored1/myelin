import { invoke } from '@tauri-apps/api/core';
import type { NoteDocument, NoteSnapshot } from '$lib/types';
import { canApplyReconciledNote, editorNeedsAuthoritativeBody, hasNoteMutation } from '$lib/noteMutation';

type ChatTool = { name: string; details: string };

/** Coordinates request lifecycle, retry/rewind, persistence, and tool approvals. */
export function createChatSession(ctx: Record<string, any>) {
	async function stopActiveChat(): Promise<boolean> {
		if (!ctx.activeChatRequestId && !ctx.isChatStreaming) return true;
		try {
			await invoke('cancel_ai');
		} catch (error) {
			console.error('Failed to stop AI:', error);
			return false;
		}
		const deadline = Date.now() + 10_000;
		while ((ctx.activeChatRequestId || ctx.isChatStreaming) && Date.now() < deadline) {
			await new Promise((resolve) => setTimeout(resolve, 50));
		}
		if (ctx.activeChatRequestId || ctx.isChatStreaming) {
			console.error('AI request did not stop within 10 seconds');
			return false;
		}
		return true;
	}

	function stopChat() {
		if (!ctx.isChatStreaming && !ctx.activeChatRequestId) return;
		void stopActiveChat();
	}

	function beginAiRequest(
		requestId: string,
		composerMode: 'chat' | 'editor',
		statusText: string,
		aiNoteId: string
	): number {
		const startTime = Date.now();
		ctx.pendingDebugTrace = [
			{ time: startTime, msg: 'Request sent', kind: 'send' },
			{ time: startTime, msg: `Composer mode: ${composerMode}`, kind: 'config' }
		];
		ctx.debugInfo = ctx.showDebugWindow
			? {
					requestStart: startTime,
					firstChunk: null,
					generationStart: null,
					generationEnd: null,
					done: null,
					promptTokens: 0,
					completionTokens: 0,
					totalTokens: 0,
					turnCount: 0,
					replyChars: 0,
					trace: ctx.pendingDebugTrace
				}
			: null;
		ctx.chatMessages = [
			...ctx.chatMessages,
			{ role: 'assistant', content: '', isStreaming: true, startTime, statusText }
		];
		ctx.activeAiComposerMode = composerMode;
		ctx.activeChatRequestId = requestId;
		ctx.activeChatNoteId = aiNoteId;
		setTimeout(() => ctx.scrollChatToBottom(true), 50);
		return startTime;
	}

	async function sendChatMessage() {
		if (!ctx.note || !ctx.chatInput.trim() || ctx.isChatStreaming) return;
		if (ctx.aiInteractionMode === 'write' && !ctx.armedEditTarget()) {
			ctx.writeTargetNotice = true;
			return;
		}
		const userText = ctx.chatInput.trim();
		ctx.chatInput = '';
		if (ctx.chatTextareaEl) ctx.chatTextareaEl.style.height = 'auto';
		await sendChatText(userText);
	}

	async function sendChatText(userText: string) {
		if (!ctx.note) return;
		const editorTarget = ctx.armedEditTarget();
		if (ctx.aiInteractionMode === 'write' && !editorTarget) {
			ctx.writeTargetNotice = true;
			if (!ctx.chatInput.trim()) ctx.chatInput = userText;
			await ctx.tick();
			if (ctx.chatTextareaEl) {
				ctx.chatTextareaEl.style.height = 'auto';
				ctx.chatTextareaEl.style.height = `${Math.min(ctx.chatTextareaEl.scrollHeight + 2, 150)}px`;
				ctx.chatTextareaEl.focus();
			}
			return;
		}
		if ((ctx.isSourceMaterial && ctx.showAttachedNote) || ctx.saveStatus !== 'saved') await ctx.saveNote();
		if (ctx.pdfIngestionPromise) await ctx.pdfIngestionPromise;
		const aiNoteId = ctx.activeAiNoteId();
		if (!aiNoteId) {
			ctx.message = 'Open or create the attached note before asking the AI to edit it.';
			return;
		}
		const requestId = Date.now().toString();
		const composerMode = ctx.aiInteractionMode === 'write' ? 'editor' : 'chat';
		const selection = composerMode === 'editor' ? editorTarget : editorTarget?.cursor ? null : editorTarget;
		ctx.activeAiEditTarget = composerMode === 'editor' ? selection : null;
		const snapshot: NoteSnapshot = {
			noteBody: ctx.draftBody,
			draftTitle: ctx.draftTitle,
			draftTags: ctx.draftTags,
			chatLength: ctx.chatMessages.length
		};
		ctx.chatMessages = [...ctx.chatMessages, { role: 'user', content: userText, snapshotId: requestId, snapshot }];
		beginAiRequest(
			requestId,
			composerMode,
			composerMode === 'editor' ? 'Preparing note edit…' : 'Retrieving note and PDF context…',
			aiNoteId
		);
		ctx.checkpointChatHistory(0);
		try {
			await invoke('ask_ai_stream', {
				noteId: aiNoteId,
				question: userText,
				requestId,
				docType: ctx.workingDocType,
				selection,
				interactionMode: ctx.aiInteractionMode,
				activeSection: ctx.activeSection
			});
		} catch (e) {
			console.error('AI Error:', e);
			ctx.failStreamingChatMessage(requestId, extractChatErrorMessage(e));
		}
	}

	async function rewindToSnapshot(snapshot?: NoteSnapshot, fillInput?: string) {
		if (!snapshot || !ctx.note) return;
		const aiNoteId = ctx.activeAiNoteId();
		if (!aiNoteId || !(await stopActiveChat())) return;
		ctx.chatMessages = ctx.chatMessages.slice(0, snapshot.chatLength);
		ctx.draftBody = snapshot.noteBody;
		ctx.draftTitle = snapshot.draftTitle;
		ctx.draftTags = snapshot.draftTags;
		ctx.note = { ...ctx.note, body: snapshot.noteBody, title: snapshot.draftTitle };
		if (ctx.vditorInstance) ctx.vditorInstance.setValue(snapshot.noteBody);
		if (fillInput !== undefined) {
			ctx.chatInput = fillInput;
			await ctx.tick();
			if (ctx.chatTextareaEl) {
				ctx.chatTextareaEl.style.height = 'auto';
				ctx.chatTextareaEl.style.height = `${Math.min(ctx.chatTextareaEl.scrollHeight, 200)}px`;
				ctx.chatTextareaEl.focus();
			}
		}
		ctx.isBusy = true;
		try {
			await invoke('save_note', {
				noteId: aiNoteId,
				title: snapshot.draftTitle,
				tags: snapshot.draftTags.split(',').map((t: string) => t.trim()).filter(Boolean),
				body: snapshot.noteBody,
				sourcePdf: ctx.activeSourceId,
				annotations: ctx.isSourceMaterial ? [] : ctx.note.annotations
			});
			await invoke('save_chat_history', { noteId: aiNoteId, chatHistory: ctx.chatMessages });
			await invoke('clear_ai_conversation', { noteId: aiNoteId });
		} catch (err) {
			console.error('Failed to rewind:', err);
		} finally {
			ctx.isBusy = false;
		}
	}

	async function retryMessage(snapshot: NoteSnapshot, userText: string) {
		await rewindToSnapshot(snapshot);
		await sendChatText(userText);
	}

	function mergeChatTools(existing: ChatTool[] = [], incoming: ChatTool[] = []) {
		const merged = [...existing];
		for (const tool of incoming) {
			if (!merged.some((entry) => entry.name === tool.name && entry.details === tool.details)) merged.push(tool);
		}
		return merged;
	}

	async function reconcileRequestNote(expectedNoteId: string) {
		if (!canApplyReconciledNote(expectedNoteId, ctx.activeAiNoteId())) return;
		const refreshed = await invoke<NoteDocument>('load_note', { noteId: expectedNoteId });
		if (!canApplyReconciledNote(expectedNoteId, ctx.activeAiNoteId())) return;
		if (!ctx.isSourceMaterial) {
			ctx.note = { ...refreshed, chatHistory: ctx.chatMessages };
			ctx.draftTitle = refreshed.title;
			ctx.draftBody = refreshed.body;
			ctx.draftTags = refreshed.tags.join(', ');
			if (ctx.workingDocType === 'md' && ctx.vditorInstance && editorNeedsAuthoritativeBody(ctx.vditorInstance.getValue(), refreshed.body)) {
				ctx.vditorInstance.setValue(refreshed.body);
			}
		} else {
			ctx.draftTitle = refreshed.title;
			ctx.draftBody = refreshed.body;
			ctx.draftTags = refreshed.tags.join(', ');
			if (ctx.vditorInstance && editorNeedsAuthoritativeBody(ctx.vditorInstance.getValue(), refreshed.body)) {
				ctx.vditorInstance.setValue(refreshed.body);
			}
		}
		void ctx.fetchRelatedNotes();
	}

	async function finishStreamingChatMessage(requestId: string, tools: ChatTool[] = []) {
		if (ctx.activeChatRequestId !== requestId) return;
		ctx.flushChatChunks();
		const requestNoteId = ctx.activeChatNoteId;
		ctx.cancelNoteStream();
		ctx.chatMessages = ctx.chatMessages.map((m: any) =>
			m.isStreaming ? { ...m, isStreaming: false, statusText: undefined, endTime: Date.now(), debugTrace: ctx.pendingDebugTrace } : m
		);
		if (ctx.chatPersistTimer) {
			clearTimeout(ctx.chatPersistTimer);
			ctx.chatPersistTimer = undefined;
		}
		if (requestNoteId) await ctx.persistChatHistory(requestNoteId, ctx.chatMessages);
		if (requestNoteId && hasNoteMutation(tools)) {
			try {
				await reconcileRequestNote(requestNoteId);
			} catch (error) {
				console.error('Failed to reconcile completed note mutation:', error);
			}
		}
		ctx.activeChatRequestId = null;
		ctx.activeAiComposerMode = null;
		ctx.activeAiEditTarget = null;
		ctx.activeChatNoteId = null;
	}

	function extractChatErrorMessage(error: unknown): string {
		if (typeof error === 'string' && error.trim()) return error;
		if (error && typeof error === 'object' && 'message' in error && typeof error.message === 'string' && error.message.trim()) return error.message;
		return 'Failed to generate response.';
	}

	function failStreamingChatMessage(requestId: string, errorMsg: string, tools: ChatTool[] = []) {
		if (ctx.activeChatRequestId !== requestId) return;
		const previewWasReverted = ctx.noteStreaming;
		if (previewWasReverted && !errorMsg.includes('Live preview reverted; no changes were saved.')) errorMsg += ' Live preview reverted; no changes were saved.';
		if (ctx.showDebugWindow && ctx.debugInfo) {
			const finishedAt = Date.now();
			ctx.debugInfo = {
				...ctx.debugInfo,
				done: finishedAt,
				generationEnd: ctx.debugInfo.generationStart ? finishedAt : ctx.debugInfo.generationEnd,
				trace: [...ctx.debugInfo.trace, { time: finishedAt, msg: `Error: ${errorMsg}`, kind: 'error' }]
			};
		}
		ctx.activeChatRequestId = null;
		ctx.activeChatNoteId = null;
		ctx.activeAiComposerMode = null;
		ctx.cancelNoteStream();
		ctx.activeAiEditTarget = null;
		ctx.chatMessages = ctx.chatMessages.map((m: any) =>
			m.isStreaming
				? { ...m, isStreaming: false, statusText: undefined, error: true, content: m.content + '\n\n' + errorMsg, tools, endTime: Date.now() }
			: m
		);
		if (ctx.chatPersistTimer) {
			clearTimeout(ctx.chatPersistTimer);
			ctx.chatPersistTimer = undefined;
		}
		const aiNoteId = ctx.activeAiNoteId();
		if (aiNoteId) void ctx.persistChatHistory(aiNoteId, ctx.chatMessages);
	}

	async function resolveApproval(id: string, approved: boolean) {
		const timeout = ctx.approvalTimeouts.get(id);
		if (timeout) {
			clearTimeout(timeout);
			ctx.approvalTimeouts.delete(id);
		}
		ctx.chatMessages = ctx.chatMessages.map((m: any) =>
			m.isApprovalRequest && m.approvalId === id ? { ...m, approvalStatus: approved ? 'approved' : 'rejected' } : m
		);
		await invoke('resolve_tool_approval', { id, approved });
	}

	return {
		stopActiveChat,
		stopChat,
		beginAiRequest,
		sendChatMessage,
		sendChatText,
		rewindToSnapshot,
		retryMessage,
		mergeChatTools,
		reconcileRequestNote,
		finishStreamingChatMessage,
		extractChatErrorMessage,
		failStreamingChatMessage,
		resolveApproval
	};
}
