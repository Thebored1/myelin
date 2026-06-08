<script lang="ts">
	import favicon from '$lib/assets/favicon.svg';
	import { onMount } from 'svelte';
	import { getCurrentWindow } from '@tauri-apps/api/window';
	import { sidebarOpen, showSidebarToggle } from '$lib/stores';

	let { children } = $props();

	let appWindow: any = null;

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

		if (typeof window !== 'undefined') {
			window.addEventListener('keydown', handleGlobalKeydown, { passive: false });
			window.addEventListener('wheel', handleGlobalWheel, { passive: false });
			return () => {
				window.removeEventListener('keydown', handleGlobalKeydown);
				window.removeEventListener('wheel', handleGlobalWheel);
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
		</div>
		<div class="titlebar-controls">
			{#if $showSidebarToggle}
				<button class="control-btn sidebar-toggle" onclick={() => $sidebarOpen = !$sidebarOpen} aria-label="Toggle sidebar" title="Toggle sidebar">
					<svg viewBox="0 0 24 24" width="12" height="12" stroke="currentColor" stroke-width="1.5" fill="none" stroke-linecap="round" stroke-linejoin="round">
						<rect x="3" y="3" width="18" height="18" rx="2" ry="2"></rect>
						<line x1="15" y1="3" x2="15" y2="21"></line>
					</svg>
				</button>
			{/if}
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
	}

	:global(html) {
		background: var(--bg-page);
		color: var(--text-primary);
		overflow: hidden;
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
		background: rgba(255, 255, 255, 0.08);
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
