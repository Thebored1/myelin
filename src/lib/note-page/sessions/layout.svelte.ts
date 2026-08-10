import { get } from 'svelte/store';
import { chatSidebarShortcut, noteSidebarOpen } from '$lib/stores';
import { shortcutMatches } from '$lib/keyboardShortcut';

/** Owns note-pane resizing, chat shortcut behavior, and chat scroll state. */
export function createLayoutSession(ctx: Record<string, any>) {
	function startSidebarResizing(e: MouseEvent) {
		e.preventDefault();
		ctx.isSidebarResizing = true;
	}

	function startResizing(e: MouseEvent) {
		e.preventDefault();
		window.getSelection()?.removeAllRanges();
		ctx.isResizing = true;
	}

	function handleGlobalMouseMove(e: MouseEvent) {
		if (ctx.isResizing && ctx.mainLayoutEl) {
			e.preventDefault();
			const rect = ctx.mainLayoutEl.getBoundingClientRect();
			const newRatio = ((e.clientX - rect.left) / rect.width) * 100;
			const maxSourceRatio = ((rect.width - ctx.PANE_MIN_WIDTH - 10) / rect.width) * 100;
			const minSourceRatio = (ctx.PANE_MIN_WIDTH / rect.width) * 100;
			if (maxSourceRatio >= minSourceRatio) ctx.splitRatio = Math.max(minSourceRatio, Math.min(newRatio, maxSourceRatio));
		} else if (ctx.isSidebarResizing) {
			const layoutRect = ctx.mainLayoutEl?.getBoundingClientRect();
			const containerWidth = layoutRect?.width ?? window.innerWidth;
			const newWidth = (layoutRect?.right ?? window.innerWidth) - e.clientX;
			const maxSidebar = Math.max(ctx.SIDEBAR_MIN_WIDTH, containerWidth - ctx.PANE_MIN_WIDTH);
			ctx.sidebarWidth = Math.max(ctx.SIDEBAR_MIN_WIDTH, Math.min(newWidth, maxSidebar));
		}
	}

	function stopResizing() {
		if (!ctx.isResizing && !ctx.isSidebarResizing) return;
		ctx.isResizing = false;
		if (ctx.isSidebarResizing) {
			ctx.isSidebarResizing = false;
			try {
				localStorage.setItem('myelin_sidebar_width', ctx.sidebarWidth.toString());
			} catch (error) {
				ctx.message = 'Sidebar width could not be saved; it will reset on the next launch.';
				console.warn('Could not save sidebar width', error);
			}
		}
		if (ctx.vditorInstance) setTimeout(() => window.dispatchEvent(new Event('resize')), 50);
	}

	async function handleChatSidebarShortcut(event: KeyboardEvent) {
		if (event.repeat || !shortcutMatches(event, get(chatSidebarShortcut)) || !ctx.note || event.defaultPrevented) return;
		event.preventDefault();
		event.stopPropagation();
		if (get(noteSidebarOpen) && document.activeElement === ctx.chatTextareaEl) {
			noteSidebarOpen.set(false);
			await ctx.tick();
			ctx.restoreShortcutEditorFocus();
			return;
		}
		ctx.captureShortcutEditorTarget();
		ctx.activeSidebarTab = 'chat';
		noteSidebarOpen.set(true);
		await ctx.tick();
		ctx.chatTextareaEl?.focus();
	}

	function handleChatScroll(e: Event) {
		const el = e.currentTarget as HTMLElement;
		ctx.userScrolledUp = el.scrollHeight - el.scrollTop - el.clientHeight > 50;
	}

	function scrollChatToBottom(force = false) {
		if (!ctx.chatMessagesEl) return;
		if (force || !ctx.userScrolledUp) ctx.chatMessagesEl.scrollTop = ctx.chatMessagesEl.scrollHeight;
	}

	$effect(() => {
		if (ctx.activeSidebarTab !== 'chat') return;
		const chatScrollKey = ctx.chatMessages.map((msg: any) => `${msg.role}:${msg.content.length}:${msg.isStreaming ? 1 : 0}:${msg.tools?.length ?? 0}:${msg.error ? 1 : 0}`).join('|');
		void chatScrollKey;
		void ctx.tick().then(() => scrollChatToBottom());
	});

	return {
		startSidebarResizing,
		startResizing,
		handleGlobalMouseMove,
		stopResizing,
		handleChatSidebarShortcut,
		handleChatScroll,
		scrollChatToBottom,
		get userScrolledUp() { return ctx.userScrolledUp; },
		set userScrolledUp(value) { ctx.userScrolledUp = value; }
	};
}
