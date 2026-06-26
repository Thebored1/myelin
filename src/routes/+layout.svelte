<script lang="ts">
	import favicon from '$lib/assets/favicon.svg';
	import { onMount } from 'svelte';
	import { getCurrentWindow } from '@tauri-apps/api/window';
	import { sidebarOpen, showSidebarToggle, noteSidebarOpen } from '$lib/stores';
	import '$lib/theme';
	import { goto } from '$app/navigation';
	import { page } from '$app/stores';

	let { children } = $props();

	let appWindow: any = null;

	let windowWidth = $state(1024);
	let wasSmallScreen = $state(false);

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
		if (typeof window !== 'undefined' && (window as any).__TAURI_INTERNALS__) {
			appWindow = getCurrentWindow();
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
			if ((e.ctrlKey || e.metaKey) && !e.shiftKey && (e.key === 'ArrowUp' || e.key === 'ArrowDown')) {
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

		if (typeof window !== 'undefined') {
			window.addEventListener('keydown', handleGlobalKeydown, { passive: false });
			window.addEventListener('wheel', handleGlobalWheel, { passive: false });
			window.addEventListener('contextmenu', handleGlobalContextMenu, { passive: false });
			return () => {
				window.removeEventListener('keydown', handleGlobalKeydown);
				window.removeEventListener('wheel', handleGlobalWheel);
				window.removeEventListener('contextmenu', handleGlobalContextMenu);
			};
		}
	});

	function minimize() {
		if (appWindow) {
			appWindow.minimize();
		}
	}

	function toggleMaximize() {
		if (appWindow) {
			appWindow.toggleMaximize();
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
	<link rel="preconnect" href="https://fonts.googleapis.com" />
	<link rel="preconnect" href="https://fonts.gstatic.com" crossorigin="anonymous" />
	<link
		href="https://fonts.googleapis.com/css2?family=Inter:wght@300;400;500;600;700&family=JetBrains+Mono:wght@400;500&display=swap"
		rel="stylesheet"
	/>
</svelte:head>

<div class="app-container">
	<!-- Custom Window Resize Handles -->
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
	<div class="resize-handle bottom-right" onmousedown={(e) => startResize('SouthEast', e)}></div>

	<header class="custom-titlebar" data-tauri-drag-region>
		<div class="titlebar-drag-region" data-tauri-drag-region>
			<img src={favicon} alt="myelin" class="titlebar-logo" data-tauri-drag-region />
			<span class="titlebar-title" data-tauri-drag-region>myelin</span>
			{#if !$page.url.pathname.startsWith('/notes/')}
			<button class="control-btn sidebar-toggle" style="margin-left: 8px; width: 32px;" onclick={() => $sidebarOpen = !$sidebarOpen} aria-label="Toggle sidebar" title="Toggle sidebar">
				<svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="1.5" fill="none" stroke-linecap="round" stroke-linejoin="round">
					<rect x="3" y="3" width="18" height="18" rx="2" ry="2"></rect>
					<line x1="9" y1="3" x2="9" y2="21"></line>
				</svg>
			</button>
			{/if}
		</div>
		<div class="titlebar-controls">
			{#if $page.url.pathname.startsWith('/notes/')}
			<button class="control-btn sidebar-toggle" onclick={() => $noteSidebarOpen = !$noteSidebarOpen} aria-label="Toggle note sidebar" title="Toggle note sidebar">
				<svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="1.5" fill="none" stroke-linecap="round" stroke-linejoin="round">
					<rect x="3" y="3" width="18" height="18" rx="2" ry="2"></rect>
					<line x1="15" y1="3" x2="15" y2="21"></line>
				</svg>
			</button>
			{/if}
			<button class="control-btn settings" onclick={() => goto('/settings')} aria-label="Settings" title="Settings">
				<svg viewBox="0 0 24 24" width="12" height="12" stroke="currentColor" stroke-width="1.5" fill="none" stroke-linecap="round" stroke-linejoin="round">
					<circle cx="12" cy="12" r="3"></circle>
					<path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1 0 2.83 2 2 0 0 1-2.83 0l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-2 2 2 2 0 0 1-2-2v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83 0 2 2 0 0 1 0-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1-2-2 2 2 0 0 1 2-2h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 0-2.83 2 2 0 0 1 2.83 0l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 2-2 2 2 0 0 1 2 2v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 0 2 2 0 0 1 0 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 2 2 2 2 0 0 1-2 2h-.09a1.65 1.65 0 0 0-1.51 1z"></path>
				</svg>
			</button>
			<button class="control-btn minimize" onclick={minimize} aria-label="Minimize" title="Minimize">
				<svg width="12" height="12" viewBox="0 0 12 12">
					<line x1="2" y1="6" x2="10" y2="6" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/>
				</svg>
			</button>
			<button class="control-btn maximize" onclick={toggleMaximize} aria-label="Maximize" title="Maximize">
				<svg width="12" height="12" viewBox="0 0 12 12">
					<rect x="2.5" y="2.5" width="7" height="7" fill="none" stroke="currentColor" stroke-width="1.2" rx="0.5"/>
				</svg>
			</button>
			<button class="control-btn close" onclick={close} aria-label="Close" title="Close">
				<svg width="12" height="12" viewBox="0 0 12 12">
					<line x1="2.5" y1="2.5" x2="9.5" y2="9.5" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/>
					<line x1="9.5" y1="2.5" x2="2.5" y2="9.5" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/>
				</svg>
			</button>
		</div>
	</header>
	<main class="app-content">
		{@render children()}
	</main>
</div>

<style>
	:global(:root) {
		--accent-100: #ef6f2e;
		--accent-200: #ee6018;
		--accent-300: #d15010;
		--surface-dark-primary: #020202;
		--surface-dark-secondary: #101010;
		--surface-light-primary: #eeeeee;
		--surface-light-secondary: #fafafa;
		--neutral-100: #d6d3d2;
		--neutral-200: #ccc9c7;
		--neutral-300: #b8b3b0;
		--neutral-400: #a49d9a;
		--neutral-500: #8a8380;
		--neutral-600: #5c5855;
		--neutral-700: #4d4947;
		--neutral-800: #3d3a39;
		--neutral-900: #2e2c2b;
		--neutral-1000: #1f1d1c;
		--text-primary: #eeeeee;
		--text-secondary: #a49d9a;
		--text-inverse: #020202;
		--text-hero: #f6f1e7;
		--border-default: #3d3a39;
		--border-subtle: #4d4947;
		--bg-page: #020202;
		--bg-panel: #101010;
		--bg-code: #1f1d1c;
		--bg-selection: #ef6f2e;
		--font-sans: 'Inter', system-ui, -apple-system, sans-serif;
		--font-mono: 'JetBrains Mono', 'Cascadia Code', 'Fira Code', monospace;
		--space-1: 0.25rem;
		--space-2: 0.5rem;
		--space-3: 0.75rem;
		--space-4: 1rem;
		--space-5: 1.25rem;
		--space-6: 1.5rem;
		--space-8: 2rem;
		--space-10: 2.5rem;
		--space-12: 3rem;
		--space-16: 4rem;
		--space-20: 5rem;
		--radius-xs: 0.125rem;
		--radius-sm: 0.1875rem;
		--radius-md: 0.25rem;
		--radius-lg: 0.375rem;
		--radius-xl: 0.5rem;
		--radius-2xl: 0.625rem;
		--radius-3xl: 0.75rem;
		--duration-fast: 0.15s;
		--duration-page: 0.32s;
		--duration-content: 0.64s;
		--ease-standard: cubic-bezier(0.4, 0, 0.2, 1);
		--ease-out: cubic-bezier(0, 0, 0.2, 1);
		--ease-emphasis: cubic-bezier(0.22, 1, 0.36, 1);
		--blur-sm: 8px;
		--blur-md: 12px;
		--blur-xl: 24px;

		/* ── Theme-aware semantic tokens ──
		   These have a dark default here and are overridden under
		   [data-theme='light'] below. Components should use these instead of
		   hardcoding rgba(255,255,255,…) overlays or opaque dark hexes, so that
		   both themes stay consistent. */
		--hover-overlay: rgba(255, 255, 255, 0.04); /* subtle row/button hover */
		--hover-overlay-strong: rgba(255, 255, 255, 0.08); /* icon-button hover */
		--overlay-faint: rgba(255, 255, 255, 0.02); /* faint chip/badge fill */
		--bg-elevated: #262626; /* raised card (e.g. note tiles) */
		--bg-elevated-hover: #333333;
		--bg-input: #1e1e1e; /* search / input surface that differs from panel */
		--bg-modal: #151515; /* modal sheet surface */
		--scrim: rgba(0, 0, 0, 0.6); /* modal backdrop */
		--scrim-soft: rgba(0, 0, 0, 0.5);
		--shadow-color: rgba(0, 0, 0, 0.4);
		--shadow-color-strong: rgba(0, 0, 0, 0.8);
		--accent-tint: rgba(238, 96, 24, 0.12); /* selected/hover accent wash */
		--danger: #e05555;
		--danger-tint: rgba(224, 85, 85, 0.1);
		--success: #4caf50;
		--on-accent: #ffffff; /* text/icons sitting on an accent fill */
		/* Status tints — translucent, so they read on either theme. */
		--info-border: rgba(100, 181, 246, 0.3);
		--info-fill: rgba(100, 181, 246, 0.06);
		--success-border: rgba(129, 199, 132, 0.3);
		--success-fill: rgba(129, 199, 132, 0.06);
		--bg-panel-blur: rgba(16, 16, 16, 0.94); /* frosted header / editor toolbar */
		--danger-text: #fecaca; /* readable text on a danger tint */
		--danger-bg: rgba(239, 68, 68, 0.12);
		--danger-bg-strong: rgba(239, 68, 68, 0.18);
		--danger-border: rgba(239, 68, 68, 0.35);
	}

	/* ── Light theme ──
	   Warm off-white surfaces with the same orange accent, matching the
	   reference. The neutral scale is inverted (100 was lightest in dark; here
	   it's darkest) so existing var(--neutral-*) usages keep their semantic
	   meaning: low numbers = prominent text, high numbers = faint borders. */
	:global(:root[data-theme='light']) {
		--surface-dark-primary: #ffffff;
		--surface-dark-secondary: #f4f2ef;
		--surface-light-primary: #1f1d1c;
		--surface-light-secondary: #2e2c2b;

		--neutral-100: #1f1d1c;
		--neutral-200: #2e2c2b;
		--neutral-300: #3d3a39;
		--neutral-400: #4d4947;
		--neutral-500: #5c5855;
		--neutral-600: #8a8380;
		--neutral-700: #a49d9a;
		--neutral-800: #ccc9c7;
		--neutral-900: #ddd9d5;
		--neutral-1000: #e8e4e0;

		--text-primary: #1f1d1c;
		--text-secondary: #6e6a67;
		--text-inverse: #ffffff;
		--text-hero: #1a1714;

		--border-default: #e2ded9;
		--border-subtle: #ece9e5;

		--bg-page: #f4f2ef;
		--bg-panel: #ffffff;
		--bg-code: #f0ede9;
		/* --bg-selection / accent stay the same orange */

		--hover-overlay: rgba(0, 0, 0, 0.04);
		--hover-overlay-strong: rgba(0, 0, 0, 0.07);
		--overlay-faint: rgba(0, 0, 0, 0.025);
		--bg-elevated: #ffffff;
		--bg-elevated-hover: #f3f0ec;
		--bg-input: #ffffff;
		--bg-modal: #ffffff;
		--scrim: rgba(40, 32, 24, 0.25);
		--scrim-soft: rgba(40, 32, 24, 0.2);
		--shadow-color: rgba(60, 50, 40, 0.12);
		--shadow-color-strong: rgba(60, 50, 40, 0.2);
		--accent-tint: rgba(238, 96, 24, 0.1);
		--bg-panel-blur: rgba(255, 255, 255, 0.9);
		--danger-text: #b42318;
		--danger-bg: rgba(239, 68, 68, 0.1);
		--danger-bg-strong: rgba(239, 68, 68, 0.16);
		--danger-border: rgba(239, 68, 68, 0.4);
		/* --danger / --success / --on-accent stay readable on both themes */
	}

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
			radial-gradient(circle at top right, rgba(239, 111, 46, 0.12), transparent 20rem),
			linear-gradient(180deg, #050505 0%, #020202 100%);
		color: var(--text-primary);
		-webkit-font-smoothing: antialiased;
		height: 100vh;
		overflow: hidden;
	}

	/* Light: warm off-white with a faint diagonal hatch (matches the reference). */
	:global(:root[data-theme='light'] body) {
		background:
			repeating-linear-gradient(
				135deg,
				rgba(120, 100, 80, 0.022) 0,
				rgba(120, 100, 80, 0.022) 1px,
				transparent 1px,
				transparent 7px
			),
			radial-gradient(circle at top right, rgba(239, 111, 46, 0.06), transparent 22rem),
			linear-gradient(180deg, #f6f4f1 0%, #f1eee9 100%);
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

	:global(::selection) {
		background: var(--bg-selection);
		color: var(--text-inverse);
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
		transition: background var(--duration-fast), color var(--duration-fast);
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
