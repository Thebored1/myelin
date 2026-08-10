import { onDestroy, onMount } from 'svelte';
import { page } from '$app/state';
import { showSidebarToggle } from '$lib/stores';
import { noteClosed } from '$lib/llamaWarm';
import { installAiEventBridge } from './aiEvents.svelte';

/** Owns route loading, DOM listeners, AI event wiring, and note teardown. */
export function createNotePageLifecycle(ctx: Record<string, any>) {
	let debugTraceEl = $state<HTMLDivElement | undefined>();

	onMount(() => {
		let savedInteractionMode: string | null = null;
		let savedSidebarWidth: string | null = null;
		try {
			savedInteractionMode = localStorage.getItem('myelin_ai_interaction_mode');
			savedSidebarWidth = localStorage.getItem('myelin_sidebar_width');
		} catch (error) {
			ctx.message = 'Browser preferences could not be read; changes remain available for this session.';
			console.warn('Could not read note preferences', error);
		}
		ctx.aiInteractionMode = savedInteractionMode === 'operation' || savedInteractionMode === 'write' ? 'write' : 'chat';
		if (savedSidebarWidth) {
			const parsed = parseInt(savedSidebarWidth, 10);
			if (!isNaN(parsed)) {
				const maxSidebar = Math.max(ctx.SIDEBAR_MIN_WIDTH, window.innerWidth - ctx.PANE_MIN_WIDTH);
				ctx.sidebarWidth = Math.max(ctx.SIDEBAR_MIN_WIDTH, Math.min(parsed, maxSidebar));
			}
		}
		showSidebarToggle.set(true);
		const mql = window.matchMedia('(max-width: 1200px)');
		const handleMediaChange = (_event: MediaQueryListEvent) => {};
		mql.addEventListener('change', handleMediaChange);
		document.addEventListener('selectionchange', ctx.handleGlobalSelectionChange);
		document.addEventListener('mousedown', ctx.onDocMouseDown, true);
		window.addEventListener('keydown', ctx.handleChatSidebarShortcut, true);
		const disposeAiEvents = installAiEventBridge(ctx.aiEventContext);
		window.addEventListener('mousemove', ctx.handleGlobalMouseMove);
		window.addEventListener('mouseup', ctx.stopResizing);
		window.addEventListener('beforeunload', ctx.handleBeforeUnload);
		return () => {
			mql.removeEventListener('change', handleMediaChange);
			document.removeEventListener('selectionchange', ctx.handleGlobalSelectionChange);
			document.removeEventListener('mousedown', ctx.onDocMouseDown, true);
			window.removeEventListener('keydown', ctx.handleChatSidebarShortcut, true);
			window.removeEventListener('mousemove', ctx.handleGlobalMouseMove);
			window.removeEventListener('mouseup', ctx.stopResizing);
			window.removeEventListener('beforeunload', ctx.handleBeforeUnload);
			showSidebarToggle.set(false);
			disposeAiEvents();
		};
	});

	onDestroy(() => {
		if (ctx.chatPersistTimer) clearTimeout(ctx.chatPersistTimer);
		if (ctx.texAutoTimer) clearTimeout(ctx.texAutoTimer);
		for (const timeout of ctx.approvalTimeouts.values()) clearTimeout(timeout);
		ctx.approvalTimeouts.clear();
		const aiNoteId = ctx.activeAiNoteId();
		if (aiNoteId && ctx.chatMessages.length) void ctx.persistChatHistory(aiNoteId, ctx.chatMessages);
		noteClosed();
		if (ctx.toolbarResizeObserver) ctx.toolbarResizeObserver.disconnect();
		ctx.editorSession.dispose();
		ctx.sourceSession.dispose();
		ctx.destroyEditorInstance();
	});

	$effect(() => {
		const routeNoteId = page.params.id;
		if (!routeNoteId || routeNoteId === ctx.loadedRouteNoteId) return;
		ctx.loadedRouteNoteId = routeNoteId;
		void ctx.loadCurrentNote(routeNoteId);
	});

	$effect(() => {
		if (ctx.debugInfo && debugTraceEl) debugTraceEl.scrollTop = debugTraceEl.scrollHeight;
	});
	$effect(() => {
		if (!ctx.showDebugWindow || !ctx.debugInfo || ctx.debugInfo.done || !ctx.activeChatRequestId) {
			if (ctx.debugTimer) {
				clearInterval(ctx.debugTimer);
				ctx.debugTimer = null;
			}
			return;
		}
		if (!ctx.debugTimer) {
			ctx.debugTimer = setInterval(() => {
				if (ctx.debugInfo && !ctx.debugInfo.done) ctx.debugInfo = { ...ctx.debugInfo };
			}, 100);
		}
	});

	return {
		get debugTraceEl() { return debugTraceEl; },
		set debugTraceEl(value: HTMLDivElement | undefined) { debugTraceEl = value; }
	};
}
