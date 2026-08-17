import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type { ChatMessage } from '$lib/types';
import type { NotePageContext } from '$lib/controller-context';

/** The mutable view-model boundary consumed by the AI event bridge. */
export type AiEventContext = NotePageContext;

/** Subscribe to all AI and LaTeX progress events used by the note page. */
export function installAiEventBridge(ctx: AiEventContext): () => void {
	let unlistenChunk: UnlistenFn | undefined;
	let unlistenDone: UnlistenFn | undefined;
	let unlistenError: UnlistenFn | undefined;
	let unlistenUsage: UnlistenFn | undefined;
	let unlistenApproval: UnlistenFn | undefined;
	let unlistenNoteWritten: UnlistenFn | undefined;
	let unlistenNoteStreamStart: UnlistenFn | undefined;
	let unlistenNoteDelta: UnlistenFn | undefined;
	let unlistenNoteStreamCancel: UnlistenFn | undefined;
	let unlistenLatex: UnlistenFn | undefined;
	let unlistenAiWarmup: UnlistenFn | undefined;
	let unlistenTool: UnlistenFn | undefined;
	listen<{ noteId: string; content: string; mode: 'write' | 'append' }>(
		'ai://note_written',
		(event) => {
			const { noteId, content, mode } = event.payload;
			if (!ctx.note || ctx.activeAiNoteId() !== noteId) return;
			ctx.applyNoteWrite(content, mode);
			if (ctx.activeChatRequestId && ctx.showDebugWindow && ctx.debugInfo) {
				ctx.debugInfo = {
					...ctx.debugInfo,
					trace: [
						...ctx.debugInfo.trace,
						{
							time: Date.now(),
							msg: `Note written (${content.length}c ${mode})`,
							kind: 'note' as const
						}
					]
				};
			}
			// Cursor anchors are consumed by an insertion. Selected spans are
			// refreshed so an immediate follow-up can target the replacement.
			if (ctx.armedSelection?.cursor || ctx.workingDocType !== 'md') ctx.clearArmedSelection();
			else if (ctx.armedSelection) setTimeout(ctx.reselectAfterEdit, 60);
		}
	).then((fn) => (unlistenNoteWritten = fn));

	listen<{ noteId: string; requestId: string }>('ai://note_stream_start', (event) => {
		if (
			!ctx.note ||
			ctx.activeAiNoteId() !== event.payload.noteId ||
			ctx.activeChatRequestId !== event.payload.requestId
		)
			return;
		ctx.beginNoteStream();
		if (ctx.showDebugWindow && ctx.debugInfo) {
			ctx.debugInfo = {
				...ctx.debugInfo,
				trace: [
					...ctx.debugInfo.trace,
					{ time: Date.now(), msg: 'Streaming note to editor…', kind: 'note' as const }
				]
			};
		}
	}).then((fn) => (unlistenNoteStreamStart = fn));

	listen<{ noteId: string; requestId: string; delta: string }>('ai://note_delta', (event) => {
		if (
			!ctx.note ||
			ctx.activeAiNoteId() !== event.payload.noteId ||
			ctx.activeChatRequestId !== event.payload.requestId
		)
			return;
		ctx.appendNoteStream(event.payload.delta);
		ctx.setStreamingStatus('Writing replacement…');
	}).then((fn) => (unlistenNoteDelta = fn));

	listen<{ noteId: string; requestId: string }>('ai://note_stream_cancel', (event) => {
		if (
			!ctx.note ||
			ctx.activeAiNoteId() !== event.payload.noteId ||
			ctx.activeChatRequestId !== event.payload.requestId
		)
			return;
		ctx.cancelNoteStream();
	}).then((fn) => (unlistenNoteStreamCancel = fn));

	listen<{ tool: string; details: string; mutatesNote?: boolean }>('ai://chat_tool', (event) => {
		if (!ctx.activeChatRequestId) return;
		let lastStartTime = Date.now();
		const toolStatus =
			ctx.activeAiComposerMode === 'editor' && event.payload.mutatesNote
				? 'Applying selected edit…'
				: `Using ${event.payload.tool.toLowerCase()}…`;
		if (ctx.showDebugWindow && ctx.debugInfo) {
			ctx.debugInfo = {
				...ctx.debugInfo,
				trace: [
					...ctx.debugInfo.trace,
					{ time: Date.now(), msg: `Tool: ${event.payload.tool}`, kind: 'tool' as const }
				]
			};
		}
		ctx.chatMessages = ctx.chatMessages.map((m: ChatMessage) => {
			if (m.isStreaming) {
				lastStartTime = m.startTime || lastStartTime;
				// On a note edit, drop the model's pre-tool prose — it tends to
				// duplicate the note content that's already shown in the editor.
				return { ...m, isStreaming: false, content: event.payload.mutatesNote ? '' : m.content };
			}
			return m;
		});
		ctx.chatMessages = [
			...ctx.chatMessages,
			{
				role: 'assistant',
				content: '',
				tools: [{ name: event.payload.tool, details: event.payload.details }],
				isStreaming: false
			},
			{
				role: 'assistant',
				content: '',
				isStreaming: true,
				startTime: lastStartTime,
				statusText: toolStatus
			}
		];
		if (ctx.chatMessagesEl) {
			setTimeout(() => ctx.scrollChatToBottom(true), 100);
		}
	}).then((fn) => (unlistenTool = fn));

	listen<{ id: string; tool: string; title: string; content: string }>(
		'ai://tool_approval_request',
		(event) => {
			// Auto-reject if the user never answers: the backend refuses the
			// request on the same deadline, so the frontend must not show a
			// stale "pending" bar forever.
			ctx.approvalTimeouts.set(
				event.payload.id,
				setTimeout(() => {
					const pending = ctx.chatMessages.find(
						(m: ChatMessage) => m.isApprovalRequest && m.approvalId === event.payload.id
					);
					if (pending && pending.approvalStatus === 'pending') {
						void ctx.resolveApproval(event.payload.id, false);
					}
				}, ctx.APPROVAL_TIMEOUT_MS)
			);
			let lastStartTime = Date.now();
			ctx.chatMessages = ctx.chatMessages.map((m: ChatMessage) => {
				if (m.isStreaming) {
					lastStartTime = m.startTime || lastStartTime;
					return { ...m, isStreaming: false };
				}
				return m;
			});
			ctx.chatMessages = [
				...ctx.chatMessages,
				{
					role: 'assistant',
					content: '',
					isApprovalRequest: true,
					approvalId: event.payload.id,
					approvalTool: event.payload.tool,
					approvalDetails: `Title: ${event.payload.title}\nContent:\n${event.payload.content}`,
					approvalStatus: 'pending'
				},
				{
					role: 'assistant',
					content: '',
					isStreaming: true,
					startTime: lastStartTime
				}
			];
			if (ctx.chatMessagesEl) {
				setTimeout(() => {
					ctx.scrollChatToBottom(true);
				}, 100);
			}
		}
	).then((fn) => (unlistenApproval = fn));

	listen<{ delta: string; requestId: string }>('ai://chat_chunk', (event) => {
		if (ctx.activeChatRequestId !== event.payload.requestId) return;
		// Buffer deltas and apply once per frame; applying on every token
		// re-renders the whole streaming bubble (full markdown re-parse).
		ctx.chatChunkBuf += event.payload.delta;
		if (!ctx.chatChunkFlushPending) {
			ctx.chatChunkFlushPending = true;
			requestAnimationFrame(() => {
				ctx.chatChunkFlushPending = false;
				ctx.flushChatChunks();
			});
		}
	}).then((fn) => (unlistenChunk = fn));

	listen<{ requestId: string; tools?: { name: string; details: string }[] }>(
		'ai://chat_done',
		(event) => {
			void ctx.finishStreamingChatMessage(event.payload.requestId, event.payload.tools || []);
			if (
				ctx.activeChatRequestId === event.payload.requestId &&
				ctx.showDebugWindow &&
				ctx.debugInfo
			) {
				ctx.debugInfo = {
					...ctx.debugInfo,
					done: Date.now(),
					trace: [...ctx.debugInfo.trace, { time: Date.now(), msg: 'Done', kind: 'done' as const }]
				};
			}
		}
	).then((fn) => (unlistenDone = fn));

	listen<{
		requestId: string;
		promptTokens: number;
		completionTokens: number;
		totalTokens: number;
	}>('ai://chat_usage', (event) => {
		if (ctx.activeChatRequestId !== event.payload.requestId) return;
		if (ctx.showDebugWindow && ctx.debugInfo) {
			ctx.debugInfo = {
				...ctx.debugInfo,
				promptTokens: event.payload.promptTokens,
				completionTokens: event.payload.completionTokens,
				totalTokens: event.payload.totalTokens
			};
			ctx.debugInfo.generationEnd = Date.now();
		}
	}).then((fn) => (unlistenUsage = fn));

	listen<{ requestId: string; message: string; tools?: { name: string; details: string }[] }>(
		'ai://chat_error',
		(event) => {
			ctx.failStreamingChatMessage(
				event.payload.requestId,
				event.payload.message,
				event.payload.tools || []
			);
		}
	).then((fn) => (unlistenError = fn));

	listen<{ status: 'started' | 'ready' | 'failed'; message?: string }>(
		'ai://llama_warmup',
		(event) => {
			if (!ctx.activeChatRequestId) return;
			if (event.payload.status === 'started') {
				ctx.setStreamingStatus('Reading the note… warming the model…');
			} else if (event.payload.status === 'ready') {
				ctx.setStreamingStatus('Model ready — preparing the response…');
			}
		}
	).then((fn) => (unlistenAiWarmup = fn));

	// Whole-document section pre-cache progress (shown over the document pane).
	let unlistenSectionCache: UnlistenFn | undefined;
	listen<{
		noteId: string;
		done: number;
		total: number;
		sectionDone?: number;
		sectionTotal?: number;
		label: string;
		profile?: 'shared' | 'chat' | 'write' | '';
		failed?: number;
		failedDetails?: string[];
		finished?: boolean;
	}>('ai://section_cache_progress', (event) => {
		const {
			done,
			total,
			sectionDone = 0,
			sectionTotal = Math.max(1, total),
			label,
			profile = '',
			failed = 0,
			failedDetails = [],
			finished = false
		} = event.payload;
		if (finished || done >= total) {
			if (event.payload.noteId !== ctx.activeAiNoteId()) return;
			if (!ctx.sectionCache) return;
			ctx.sectionCache = {
				...ctx.sectionCache,
				done,
				total: Math.max(total, 1),
				sectionDone,
				sectionTotal,
				label,
				profile,
				failed,
				failedDetails,
				finished: true,
				elapsedMs: Math.max(0, performance.now() - ctx.sectionCache.startedAt)
			};
			return;
		}
		if (event.payload.noteId !== ctx.activeAiNoteId()) return;
		ctx.sectionCache = {
			...(ctx.sectionCache ?? {
				done: 0,
				total: Math.max(total, 1),
				sectionDone: 0,
				sectionTotal,
				label: '',
				profile: '',
				startedAt: performance.now(),
				finished: false,
				failed: 0,
				failedDetails: [],
				elapsedMs: null
			}),
			done: Math.max(done, 0),
			total: Math.max(total, 1),
			sectionDone,
			sectionTotal,
			label,
			profile,
			failed,
			failedDetails,
			finished: false,
			elapsedMs: null
		};
	}).then((fn) => (unlistenSectionCache = fn));

	// Debug event: model behavior, tool calls, grammar config, etc.
	let unlistenDebug: UnlistenFn | undefined;
	listen<{ kind: string; msg: string; requestId: string }>('ai://debug_event', (event) => {
		if (ctx.activeChatRequestId !== event.payload.requestId) return;
		const sensitivePrompt = event.payload.kind === 'model_prompt';
		const message = sensitivePrompt
			? `Model prompt omitted from diagnostics (${event.payload.msg.length} characters).`
			: event.payload.msg;
		const entry = ctx.makeDebugTraceEntry(event.payload.kind, message);
		// Keep bounded operational telemetry even with the panel closed, but never
		// persist full model prompts: they can contain the note body, selections,
		// retrieved passages, or credentials embedded in user-provided context.
		ctx.pendingDebugTrace = [...ctx.pendingDebugTrace.slice(-(ctx.MAX_DEBUG_TRACE - 1)), entry];
		const status = ctx.visibleAiStatus(event.payload.kind, event.payload.msg);
		if (status) ctx.setStreamingStatus(status);
		if (ctx.showDebugWindow && ctx.debugInfo) {
			const isModelStart =
				event.payload.kind === 'gen' || event.payload.kind === 'first_model_delta';
			ctx.debugInfo = {
				...ctx.debugInfo,
				generationStart: isModelStart ? entry.time : ctx.debugInfo.generationStart,
				firstChunk:
					event.payload.kind === 'gen' && ctx.debugInfo.firstChunk === null
						? entry.time
						: ctx.debugInfo.firstChunk,
				trace: [...ctx.debugInfo.trace.slice(-(ctx.MAX_DEBUG_TRACE - 1)), entry]
			};
		}
	}).then((fn) => (unlistenDebug = fn));

	// LaTeX support bundle download progress (first compile only).
	listen<{ phase: string; bytes?: number; message?: string }>('latex://download', (event) => {
		const p = event.payload;
		const mb = ((p.bytes ?? 0) / (1024 * 1024)).toFixed(1);
		if (p.phase === 'start' || p.phase === 'progress') {
			ctx.latexDownloadMsg = `Downloading LaTeX support files (first run)… ${mb} MB`;
		} else if (p.phase === 'done') {
			ctx.texCacheWarmed = true;
			ctx.latexDownloadMsg = null;
		} else if (p.phase === 'error') {
			ctx.latexDownloadMsg = null;
		}
	}).then((fn) => (unlistenLatex = fn));
	invoke<{ warmed: boolean }>('tectonic_cache_status')
		.then((status) => (ctx.texCacheWarmed = status.warmed))
		.catch(() => {});

	return () => {
		if (unlistenChunk) unlistenChunk();
		if (unlistenDone) unlistenDone();
		if (unlistenError) unlistenError();
		if (unlistenUsage) unlistenUsage();
		if (unlistenTool) unlistenTool();
		if (unlistenApproval) unlistenApproval();
		if (unlistenDebug) unlistenDebug();
		if (unlistenNoteWritten) unlistenNoteWritten();
		if (unlistenNoteStreamStart) unlistenNoteStreamStart();
		if (unlistenNoteDelta) unlistenNoteDelta();
		if (unlistenNoteStreamCancel) unlistenNoteStreamCancel();
		if (unlistenLatex) unlistenLatex();
		if (unlistenAiWarmup) unlistenAiWarmup();
		if (unlistenSectionCache) unlistenSectionCache();
	};
}
