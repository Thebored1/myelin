<script lang="ts">
	import favicon from '$lib/assets/favicon.svg';
	import { onMount } from 'svelte';
	import { getCurrentWindow } from '@tauri-apps/api/window';
	import { invoke } from '@tauri-apps/api/core';
	import { listen } from '@tauri-apps/api/event';
	import { sidebarOpen, showSidebarToggle, noteSidebarOpen } from '$lib/stores';
	import { initializeThemes } from '$lib/theme';
	import { goto } from '$app/navigation';
	import { page } from '$app/stores';
	import { providerAiStatus, type AiStatus } from '$lib/aiStatus';
	import '@fontsource-variable/inter';
	import '@fontsource-variable/jetbrains-mono';
	import '$lib/styles/theme.css';

	let { children } = $props();

	let appWindow: any = null;

	let windowWidth = $state(1024);
	let wasSmallScreen = $state(false);
	let isWindowMaximized = $state(false);
	let isWindowFullscreen = $state(false);
	let aiStatus = $state<AiStatus>('loading');

	$effect(() => {
		const isSmallScreen = windowWidth < 1200;
		if (isSmallScreen && !wasSmallScreen) {
			$sidebarOpen = false;
		} else if (!isSmallScreen && wasSmallScreen) {
			$sidebarOpen = true;
		}
		wasSmallScreen = isSmallScreen;
	});

	onMount(() => {
		void initializeThemes();
		let unlistenResize: (() => void) | undefined;
		let unlistenAiWarmup: (() => void) | undefined;
		let aiStatusPoll: ReturnType<typeof setInterval> | undefined;
		if (typeof window !== 'undefined' && (window as any).__TAURI_INTERNALS__) {
			const syncAiStatus = async () => {
				try {
					const status = await invoke<any>('get_provider_status');
					aiStatus = providerAiStatus(status, aiStatus);
				} catch {
					// Keep the warmup event as the source of truth when status is transiently unavailable.
				}
			};
			void listen<{ status: 'started' | 'ready' | 'failed'; message?: string }>(
				'ai://llama_warmup',
				(event) => {
					aiStatus = event.payload.status === 'ready' ? 'ready' : 'loading';
					if (event.payload.status === 'failed') aiStatus = 'unavailable';
				}
			).then((unlisten) => {
				unlistenAiWarmup = unlisten;
				// Reconcile after listener registration; the one-shot ready event may
				// have fired before this layout finished mounting.
				void syncAiStatus();
			});
			void syncAiStatus();
			// Keep the badge self-healing if the warmup event is missed or the server
			// restarts while the app is open.
			aiStatusPoll = setInterval(() => void syncAiStatus(), 2000);
		}
		if (typeof window !== 'undefined' && (window as any).__TAURI_INTERNALS__) {
			appWindow = getCurrentWindow();
			const syncWindowState = async () => {
				[isWindowMaximized, isWindowFullscreen] = await Promise.all([
					appWindow.isMaximized(),
					appWindow.isFullscreen()
				]);
			};
			void syncWindowState();
			void appWindow
				.onResized(() => void syncWindowState())
				.then((unlisten: () => void) => {
					unlistenResize = unlisten;
				});
		}

		// Prevent Ctrl+A globally unless focused in an input or editor
		const handleGlobalKeydown = (e: KeyboardEvent) => {
			const target = e.target as HTMLElement;
			const isInput = target.tagName === 'INPUT' || target.tagName === 'TEXTAREA';
			const isContentEditable = target.isContentEditable;

			if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'a') {
				if (!isInput && !isContentEditable) {
					e.preventDefault();
				}
			}

			// Prevent Ctrl+Arrow and plain arrow keys from scrolling the page
			const arrowKeys = ['ArrowUp', 'ArrowDown', 'ArrowLeft', 'ArrowRight'];
			if (arrowKeys.includes(e.key)) {
				if (!isInput && !isContentEditable) {
					e.preventDefault();
				}
			}

			// Prevent Ctrl+Arrow keys from scrolling (Up/Down usually scroll), but allow Shift for selection
			if (
				(e.ctrlKey || e.metaKey) &&
				!e.shiftKey &&
				(e.key === 'ArrowUp' || e.key === 'ArrowDown')
			) {
				e.preventDefault();
			}
		};

		const handleGlobalWheel = (e: WheelEvent) => {
			if (e.ctrlKey || e.metaKey) {
				e.preventDefault(); // Prevent zooming with Ctrl+Scroll
			}
		};

		const handleGlobalContextMenu = (e: MouseEvent) => {
			const target = e.target as HTMLElement;
			const isInput = target.tagName === 'INPUT' || target.tagName === 'TEXTAREA';
			const isContentEditable = target.isContentEditable;

			// Disable the default browser right-click menu unless we're in a text input
			if (!isInput && !isContentEditable) {
				e.preventDefault();
			}
		};

		// Global: clicking outside any modal <dialog> closes it. A click whose target
		// is the <dialog> element itself landed on its backdrop (the content sits in
		// child nodes), so close it.
		const handleGlobalDialogClick = (e: MouseEvent) => {
			const target = e.target as HTMLElement;
			if (target instanceof HTMLDialogElement && target.open) {
				target.close();
			}
		};

		if (typeof window !== 'undefined') {
			window.addEventListener('keydown', handleGlobalKeydown, { passive: false });
			window.addEventListener('wheel', handleGlobalWheel, { passive: false });
			window.addEventListener('contextmenu', handleGlobalContextMenu, { passive: false });
			window.addEventListener('click', handleGlobalDialogClick);
			return () => {
				window.removeEventListener('keydown', handleGlobalKeydown);
				window.removeEventListener('wheel', handleGlobalWheel);
				window.removeEventListener('contextmenu', handleGlobalContextMenu);
				window.removeEventListener('click', handleGlobalDialogClick);
				unlistenResize?.();
				unlistenAiWarmup?.();
				if (aiStatusPoll) clearInterval(aiStatusPoll);
			};
		}
	});

	function minimize() {
		if (appWindow) {
			appWindow.minimize();
		}
	}

	async function toggleMaximize() {
		if (appWindow) {
			await appWindow.toggleMaximize();
			isWindowMaximized = await appWindow.isMaximized();
			isWindowFullscreen = await appWindow.isFullscreen();
		}
	}

	function close() {
		if (appWindow) {
			appWindow.close();
		}
	}

	function startResize(direction: string, event: MouseEvent) {
		if (appWindow && event.buttons === 1) {
			appWindow.startResizeDragging(direction);
		}
	}
</script>

<svelte:window bind:innerWidth={windowWidth} />

<svelte:head>
	<link rel="icon" href={favicon} />
</svelte:head>

{#if $page.url.pathname === '/quick'}
	{@render children()}
{:else}
	<div class="app-container">
		{#if !isWindowMaximized && !isWindowFullscreen}
			<!-- Custom Window Resize Handles (hidden while maximized/fullscreen). -->
			<!-- svelte-ignore a11y_no_static_element_interactions -->
			<div class="resize-handle top" onmousedown={(e) => startResize('North', e)}></div>
			<!-- svelte-ignore a11y_no_static_element_interactions -->
			<div class="resize-handle bottom" onmousedown={(e) => startResize('South', e)}></div>
			<!-- svelte-ignore a11y_no_static_element_interactions -->
			<div class="resize-handle left" onmousedown={(e) => startResize('West', e)}></div>
			<!-- svelte-ignore a11y_no_static_element_interactions -->
			<div class="resize-handle right" onmousedown={(e) => startResize('East', e)}></div>
			<!-- svelte-ignore a11y_no_static_element_interactions -->
			<div class="resize-handle top-left" onmousedown={(e) => startResize('NorthWest', e)}></div>
			<!-- svelte-ignore a11y_no_static_element_interactions -->
			<div class="resize-handle top-right" onmousedown={(e) => startResize('NorthEast', e)}></div>
			<!-- svelte-ignore a11y_no_static_element_interactions -->
			<div class="resize-handle bottom-left" onmousedown={(e) => startResize('SouthWest', e)}></div>
			<!-- svelte-ignore a11y_no_static_element_interactions -->
			<div
				class="resize-handle bottom-right"
				onmousedown={(e) => startResize('SouthEast', e)}
			></div>
		{/if}

		<header class="custom-titlebar" data-tauri-drag-region>
			<div class="titlebar-drag-region" data-tauri-drag-region>
				<img src={favicon} alt="myelin" class="titlebar-logo" data-tauri-drag-region />
				<span class="titlebar-title" data-tauri-drag-region>myelin</span>
				{#if !$page.url.pathname.startsWith('/notes/')}
					<button
						class="control-btn sidebar-toggle"
						style="margin-left: 8px; width: 32px;"
						onclick={() => ($sidebarOpen = !$sidebarOpen)}
						aria-label="Toggle sidebar"
						title="Toggle sidebar"
					>
						<svg
							viewBox="0 0 24 24"
							width="14"
							height="14"
							stroke="currentColor"
							stroke-width="1.5"
							fill="none"
							stroke-linecap="round"
							stroke-linejoin="round"
						>
							<rect x="3" y="3" width="18" height="18" rx="2" ry="2"></rect>
							<line x1="9" y1="3" x2="9" y2="21"></line>
						</svg>
					</button>
				{/if}
			</div>
			<div class="titlebar-controls">
				<div
					class="ai-status"
					class:ready={aiStatus === 'ready'}
					class:unavailable={aiStatus === 'unavailable' || aiStatus === 'unconfigured'}
					role="status"
					aria-label={aiStatus === 'ready'
						? 'AI ready'
						: aiStatus === 'unconfigured'
							? 'No AI model selected'
							: aiStatus === 'unavailable'
								? 'AI unavailable'
								: 'AI loading'}
					title={aiStatus === 'ready'
						? 'AI ready'
						: aiStatus === 'unconfigured'
							? 'Select an AI model in Settings'
							: aiStatus === 'unavailable'
								? 'AI model could not be loaded'
								: 'AI model is loading'}
				>
					<span class="ai-status-dot"></span>
					{#if aiStatus === 'loading'}<span>AI loading</span>
					{:else if aiStatus === 'unconfigured'}<span>No model selected</span>
					{:else if aiStatus === 'unavailable'}<span>AI unavailable</span>{/if}
				</div>
				{#if $page.url.pathname.startsWith('/notes/')}
					<button
						class="control-btn sidebar-toggle"
						onclick={() => ($noteSidebarOpen = !$noteSidebarOpen)}
						aria-label="Toggle note sidebar"
						title="Toggle note sidebar"
					>
						<svg
							viewBox="0 0 24 24"
							width="14"
							height="14"
							stroke="currentColor"
							stroke-width="1.5"
							fill="none"
							stroke-linecap="round"
							stroke-linejoin="round"
						>
							<rect x="3" y="3" width="18" height="18" rx="2" ry="2"></rect>
							<line x1="15" y1="3" x2="15" y2="21"></line>
						</svg>
					</button>
				{/if}
				<button
					class="control-btn settings"
					onclick={() => goto('/settings')}
					aria-label="Settings"
					title="Settings"
				>
					<svg
						viewBox="0 0 24 24"
						width="12"
						height="12"
						stroke="currentColor"
						stroke-width="1.5"
						fill="none"
						stroke-linecap="round"
						stroke-linejoin="round"
					>
						<circle cx="12" cy="12" r="3"></circle>
						<path
							d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1 0 2.83 2 2 0 0 1-2.83 0l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-2 2 2 2 0 0 1-2-2v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83 0 2 2 0 0 1 0-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1-2-2 2 2 0 0 1 2-2h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 0-2.83 2 2 0 0 1 2.83 0l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 2-2 2 2 0 0 1 2 2v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 0 2 2 0 0 1 0 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 2 2 2 2 0 0 1-2 2h-.09a1.65 1.65 0 0 0-1.51 1z"
						></path>
					</svg>
				</button>
				<button
					class="control-btn minimize"
					onclick={minimize}
					aria-label="Minimize"
					title="Minimize"
				>
					<svg width="12" height="12" viewBox="0 0 12 12">
						<line
							x1="2"
							y1="6"
							x2="10"
							y2="6"
							stroke="currentColor"
							stroke-width="1.2"
							stroke-linecap="round"
						/>
					</svg>
				</button>
				<button
					class="control-btn maximize"
					onclick={toggleMaximize}
					aria-label="Maximize"
					title="Maximize"
				>
					<svg width="12" height="12" viewBox="0 0 12 12">
						<rect
							x="2.5"
							y="2.5"
							width="7"
							height="7"
							fill="none"
							stroke="currentColor"
							stroke-width="1.2"
							rx="0.5"
						/>
					</svg>
				</button>
				<button class="control-btn close" onclick={close} aria-label="Close" title="Close">
					<svg width="12" height="12" viewBox="0 0 12 12">
						<line
							x1="2.5"
							y1="2.5"
							x2="9.5"
							y2="9.5"
							stroke="currentColor"
							stroke-width="1.2"
							stroke-linecap="round"
						/>
						<line
							x1="9.5"
							y1="2.5"
							x2="2.5"
							y2="9.5"
							stroke="currentColor"
							stroke-width="1.2"
							stroke-linecap="round"
						/>
					</svg>
				</button>
			</div>
		</header>
		<main class="app-content">
			{@render children()}
		</main>
	</div>
{/if}

<style>
	:global(html) {
		background: var(--bg-page);
		color: var(--text-primary);
		overflow: hidden;
		font-size: 14px;
	}

	:global(body) {
		margin: 0;
		padding: 0;
		font-family: var(--font-sans);
		background:
			radial-gradient(circle at top right, var(--page-glow), transparent 20rem),
			linear-gradient(180deg, var(--page-gradient-start) 0%, var(--page-gradient-end) 100%);
		color: var(--text-primary);
		-webkit-font-smoothing: antialiased;
		height: 100vh;
		overflow: hidden;
		/* App-like feel: text isn't selectable by default. Editable surfaces
		   re-enable it below (form fields, the note editor, the PDF text layer). */
		user-select: none;
		-webkit-user-select: none;
	}

	/* Light: warm off-white with a faint diagonal hatch (matches the reference). */
	:global(:root[data-theme='light'] body) {
		background:
			repeating-linear-gradient(
				135deg,
				var(--page-pattern) 0,
				var(--page-pattern) 1px,
				transparent 1px,
				transparent 7px
			),
				radial-gradient(circle at top right, var(--page-glow), transparent 22rem),
				linear-gradient(180deg, var(--page-gradient-start) 0%, var(--page-gradient-end) 100%);
	}

	:global(input),
	:global(textarea),
	:global(select),
	:global(button),
	:global(.vditor),
	:global(.interactive) {
		font-family: var(--font-mono);
	}

	:global(*),
	:global(*::before),
	:global(*::after) {
		box-sizing: border-box;
	}

	/* Re-enable text selection only where it's genuinely needed: form fields, any
	   contenteditable, the Vditor note editor, math fields, the PDF text layer,
	   and dynamic AI/tool output. Static app controls and chrome remain
	   unselectable so the app retains its native feel. */
	:global(input),
	:global(textarea),
	:global([contenteditable]:not([contenteditable='false'])),
	:global(.vditor-ir),
	:global(.vditor-ir *),
	:global(.vditor-sv),
	:global(.vditor-sv *),
	:global(.vditor-wysiwyg),
	:global(.vditor-wysiwyg *),
	:global(.vditor-reset),
	:global(.vditor-reset *),
	:global(.vditor-textarea),
	:global(math-field),
	:global(.textLayer),
	:global(.textLayer *),
	:global(.selectable-content),
	:global(.selectable-content *) {
		user-select: text;
		-webkit-user-select: text;
	}

	/* The default WebKitGTK focus ring (drawn in the GTK theme accent color)
	   appears around every clicked control. Suppress it globally. */
	:global(:focus),
	:global(:focus-visible) {
		outline: none;
	}

	:global(::selection) {
		background: var(--bg-selection);
		color: var(--text-selection);
	}

	/* Adjust layout height for all shells to fit inside the custom height */
	:global(.shell),
	:global(.editor-shell) {
		height: calc(100vh - 32px) !important;
		min-height: calc(100vh - 32px) !important;
		max-height: calc(100vh - 32px) !important;
		overflow: hidden !important;
	}

	.app-container {
		display: flex;
		flex-direction: column;
		height: 100vh;
		width: 100vw;
		overflow: hidden;
		position: relative;
	}

	.resize-handle {
		position: absolute;
		z-index: 99999;
	}

	.resize-handle.top {
		top: 0;
		left: 6px;
		right: 6px;
		height: 6px;
		cursor: n-resize;
	}

	.resize-handle.bottom {
		bottom: 0;
		left: 6px;
		right: 6px;
		height: 6px;
		cursor: s-resize;
	}

	.resize-handle.left {
		left: 0;
		top: 6px;
		bottom: 6px;
		width: 6px;
		cursor: w-resize;
	}

	.resize-handle.right {
		right: 0;
		top: 6px;
		bottom: 6px;
		width: 6px;
		cursor: e-resize;
	}

	.resize-handle.top-left {
		top: 0;
		left: 0;
		width: 10px;
		height: 10px;
		cursor: nw-resize;
	}

	.resize-handle.top-right {
		top: 0;
		right: 0;
		width: 10px;
		height: 10px;
		cursor: ne-resize;
	}

	.resize-handle.bottom-left {
		bottom: 0;
		left: 0;
		width: 10px;
		height: 10px;
		cursor: sw-resize;
	}

	.resize-handle.bottom-right {
		bottom: 0;
		right: 0;
		width: 10px;
		height: 10px;
		cursor: se-resize;
	}

	.custom-titlebar {
		height: 32px;
		background: var(--bg-panel);
		border-bottom: 1px solid var(--border-default);
		display: flex;
		justify-content: space-between;
		align-items: center;
		user-select: none;
		-webkit-user-select: none;
		z-index: 9999;
	}

	:global(.app-container:has(.vditor--fullscreen) .custom-titlebar) {
		z-index: -1 !important;
		opacity: 0;
		pointer-events: none;
	}

	/* Vditor's editor fullscreen is still inside the app window; remove the
	   custom resize hit targets so the top edge never shows a resize cursor. */
	:global(.app-container:has(.vditor--fullscreen) .resize-handle) {
		display: none;
	}

	.titlebar-drag-region {
		flex: 1;
		height: 100%;
		display: flex;
		align-items: center;
		padding-left: var(--space-4);
		cursor: default;
	}

	.titlebar-logo {
		width: 14px;
		height: 14px;
		margin-right: var(--space-2);
		opacity: 0.8;
	}

	.titlebar-title {
		font-size: 0.75rem;
		font-family: var(--font-mono);
		font-weight: 500;
		color: var(--text-secondary);
		letter-spacing: 0.05em;
	}

	.ai-status {
		display: inline-flex;
		align-items: center;
		gap: 5px;
		justify-content: center;
		min-width: 46px;
		font-size: 0.65rem;
		font-family: var(--font-mono);
		color: #d99b43;
		letter-spacing: 0.02em;
		white-space: nowrap;
	}

	.ai-status.ready {
		color: var(--success);
	}

	.ai-status.unavailable {
		color: var(--text-secondary);
	}

	.ai-status.ready,
	.ai-status.unavailable {
		min-width: 28px;
	}

	.ai-status-dot {
		width: 6px;
		height: 6px;
		border-radius: 50%;
		background: currentColor;
		box-shadow: 0 0 0 2px color-mix(in srgb, currentColor 16%, transparent);
	}

	.ai-status:not(.ready):not(.unavailable) .ai-status-dot {
		animation: ai-status-pulse 1.2s ease-in-out infinite;
	}

	@keyframes ai-status-pulse {
		0%,
		100% {
			opacity: 0.45;
		}
		50% {
			opacity: 1;
		}
	}

	.titlebar-controls {
		display: flex;
		height: 100%;
		align-items: stretch;
	}

	.control-btn {
		background: transparent;
		border: none;
		width: 46px;
		height: 100%;
		display: flex;
		align-items: center;
		justify-content: center;
		color: var(--text-secondary);
		transition:
			background var(--duration-fast),
			color var(--duration-fast);
		padding: 0;
		cursor: pointer;
		border-radius: 0;
	}

	.control-btn:hover {
		background: var(--hover-overlay-strong);
		color: var(--text-primary);
	}

	.control-btn:hover:not(:disabled) {
		transform: none !important; /* override global button hover translate */
	}

	.control-btn.close:hover {
		background: #e81123;
		color: white;
	}

	.app-content {
		flex: 1;
		min-height: 0;
		overflow: auto;
	}
</style>
