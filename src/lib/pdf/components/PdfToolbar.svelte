<script lang="ts">
	import type { PdfAnnotation } from '$lib/types';
	import type { ScrollMode, SpreadMode, ToolMode } from '../types';
	import PdfCompactMenu from './PdfCompactMenu.svelte';

	type Props = {
		toolMode?: ToolMode;
		scrollMode?: ScrollMode;
		spreadMode?: SpreadMode;
		showDisplayMenu?: boolean;
		showMoreMenu?: boolean;
		pdfCompact?: boolean;
		activePage?: number;
		numPages?: number;
		scale?: number;
		pdfControlsEl?: HTMLElement;
		showAttachButton?: boolean;
		onAttachNote?: () => void;
		onClosePdf?: () => void;
		fitToScreen: () => void;
		changePage: (delta: number) => void;
		zoomIn: () => void;
		zoomOut: () => void;
	};
	let {
		toolMode = $bindable<ToolMode>('select'),
		scrollMode = $bindable<ScrollMode>('page'),
		spreadMode = $bindable<SpreadMode>('none'),
		showDisplayMenu = $bindable(false),
		showMoreMenu = $bindable(false),
		pdfCompact = $bindable(false),
		activePage = $bindable(0),
		numPages = $bindable(0),
		scale = $bindable(1.2),
		pdfControlsEl = $bindable<HTMLElement>(),
		showAttachButton = true,
		onAttachNote,
		onClosePdf,
		fitToScreen,
		changePage,
		zoomIn,
		zoomOut
	}: Props = $props();
</script>

<div class="pdf-controls" bind:this={pdfControlsEl}>
		{#if !pdfCompact}
			<!-- Annotation Tools -->
			<div class="tools-group">
				<button
					class:active={toolMode === 'select'}
					onclick={() => (toolMode = 'select')}
					title="Select Text"
				>
					<svg
						viewBox="0 0 24 24"
						width="14"
						height="14"
						stroke="currentColor"
						stroke-width="2"
						fill="none"
						><path d="M3 3l7.07 16.97 2.51-7.39 7.39-2.51L3 3z"></path><path d="M13 13l6 6"
						></path></svg
					>
				</button>
				<button class:active={toolMode === 'pen'} onclick={() => (toolMode = 'pen')} title="Draw">
					<svg
						viewBox="0 0 24 24"
						width="14"
						height="14"
						stroke="currentColor"
						stroke-width="2"
						fill="none"
						><path d="M12 19l7-7 3 3-7 7-3-3z"></path><path
							d="M18 13l-1.5-7.5L2 2l3.5 14.5L13 18l5-5z"
						></path><path d="M2 2l7.586 7.586"></path><circle cx="11" cy="11" r="2"></circle></svg
					>
				</button>
				<button
					class:active={toolMode === 'eraser'}
					onclick={() => (toolMode = 'eraser')}
					title="Eraser"
				>
					<svg
						viewBox="0 0 24 24"
						width="14"
						height="14"
						stroke="currentColor"
						stroke-width="2"
						fill="none"
						><path
							d="M20 20H7L3 16C2.5 15.5 2.5 14.5 3 14L13 4C13.5 3.5 14.5 3.5 15 4L20 9C20.5 9.5 20.5 10.5 20 11L11 20H20"
						></path></svg
					>
				</button>
				<button
					class:active={toolMode === 'marquee'}
					onclick={() => (toolMode = 'marquee')}
					title="Marquee / Crop"
				>
					<svg
						viewBox="0 0 24 24"
						width="14"
						height="14"
						stroke="currentColor"
						stroke-width="2"
						fill="none"
						><rect x="3" y="3" width="18" height="18" rx="2" ry="2" stroke-dasharray="4 4"
						></rect></svg
					>
				</button>
			</div>

			<div style="flex-grow: 1;"></div>

			{#if scrollMode !== 'page'}
				<span style="white-space: nowrap;">{numPages} Pages</span>
			{/if}

			{#if scrollMode === 'page'}
				<button
					class="page-nav-btn"
					onclick={() => changePage(-1)}
					disabled={activePage <= 1}
					title="Previous page"
					aria-label="Previous page"
				>
					<svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2"
						><path d="m15 18-6-6 6-6" /></svg
					>
				</button>
				<span class="page-counter">Page {activePage || 1} / {numPages}</span>
				<button
					class="page-nav-btn"
					onclick={() => changePage(1)}
					disabled={activePage >= numPages}
					title="Next page"
					aria-label="Next page"
				>
					<svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2"
						><path d="m9 18 6-6-6-6" /></svg
					>
				</button>
			{/if}

			<div class="display-menu-container">
				<button
					class="display-btn"
					onclick={(e) => {
						e.stopPropagation();
						showDisplayMenu = !showDisplayMenu;
					}}
					title="Layout Options"
				>
					<svg
						viewBox="0 0 24 24"
						width="16"
						height="16"
						stroke="currentColor"
						stroke-width="2"
						fill="none"
						style="margin-right: 4px;"
						><rect x="4" y="4" width="16" height="16" rx="2" ry="2"></rect><line
							x1="4"
							y1="12"
							x2="20"
							y2="12"
						></line></svg
					>
					Layout
				</button>
				{#if showDisplayMenu}
					<div class="display-menu dropdown">
						<div class="menu-group">
							<button
								class:active={scrollMode === 'page' && spreadMode === 'none'}
								onclick={() => {
									scrollMode = 'page';
									spreadMode = 'none';
									showDisplayMenu = false;
								}}
							>
								<svg
									viewBox="0 0 24 24"
									width="14"
									height="14"
									stroke="currentColor"
									stroke-width="2"
									fill="none"
									><rect x="7" y="3" width="10" height="18" rx="2"></rect><path d="M7 8h10M7 16h10"
									></path></svg
								>
								Single Page
							</button>
							<button
								class:active={scrollMode === 'vertical'}
								onclick={() => {
									scrollMode = 'vertical';
									showDisplayMenu = false;
								}}
							>
								<svg
									viewBox="0 0 24 24"
									width="14"
									height="14"
									stroke="currentColor"
									stroke-width="2"
									fill="none"
									><rect x="7" y="3" width="10" height="18" rx="2"></rect><path d="M12 3v18"
									></path></svg
								>
								Vertical Scrolling
							</button>
							<button
								class:active={scrollMode === 'horizontal'}
								onclick={() => {
									scrollMode = 'horizontal';
									showDisplayMenu = false;
								}}
							>
								<svg
									viewBox="0 0 24 24"
									width="14"
									height="14"
									stroke="currentColor"
									stroke-width="2"
									fill="none"
									><rect x="3" y="7" width="18" height="10" rx="2"></rect><path d="M3 12h18"
									></path></svg
								>
								Horizontal Scrolling
							</button>
							<button
								class:active={scrollMode === 'wrapped'}
								onclick={() => {
									scrollMode = 'wrapped';
									showDisplayMenu = false;
								}}
							>
								<svg
									viewBox="0 0 24 24"
									width="14"
									height="14"
									stroke="currentColor"
									stroke-width="2"
									fill="none"
									><rect x="3" y="3" width="7" height="7" rx="1"></rect><rect
										x="14"
										y="3"
										width="7"
										height="7"
										rx="1"
									></rect><rect x="3" y="14" width="7" height="7" rx="1"></rect><rect
										x="14"
										y="14"
										width="7"
										height="7"
										rx="1"
									></rect></svg
								>
								Wrapped Scrolling
							</button>
						</div>
						<div class="menu-divider"></div>
						<div class="menu-group">
							<button
								class:active={spreadMode === 'none'}
								onclick={() => {
									spreadMode = 'none';
									scrollMode = scrollMode === 'page' ? 'vertical' : scrollMode;
									showDisplayMenu = false;
								}}
							>
								<svg
									viewBox="0 0 24 24"
									width="14"
									height="14"
									stroke="currentColor"
									stroke-width="2"
									fill="none"><rect x="7" y="3" width="10" height="18" rx="2"></rect></svg
								>
								No Spreads
							</button>
							<button
								class:active={spreadMode === 'odd'}
								onclick={() => {
									spreadMode = 'odd';
									scrollMode = 'vertical';
									showDisplayMenu = false;
								}}
							>
								<svg
									viewBox="0 0 24 24"
									width="14"
									height="14"
									stroke="currentColor"
									stroke-width="2"
									fill="none"
									><rect x="3" y="3" width="8" height="18" rx="1"></rect><rect
										x="13"
										y="3"
										width="8"
										height="18"
										rx="1"
									></rect></svg
								>
								Odd Spreads
							</button>
							<button
								class:active={spreadMode === 'even'}
								onclick={() => {
									spreadMode = 'even';
									scrollMode = 'vertical';
									showDisplayMenu = false;
								}}
							>
								<svg
									viewBox="0 0 24 24"
									width="14"
									height="14"
									stroke="currentColor"
									stroke-width="2"
									fill="none"
									><rect x="3" y="3" width="8" height="18" rx="1"></rect><rect
										x="13"
										y="3"
										width="8"
										height="18"
										rx="1"
									></rect></svg
								>
								Even Spreads
							</button>
						</div>
					</div>
				{/if}
			</div>
		{/if}

		{#if pdfCompact}
			<div style="flex-grow: 1;"></div>
		{/if}

		<button onclick={fitToScreen} title="Fit to Screen">
			<svg
				viewBox="0 0 24 24"
				width="14"
				height="14"
				stroke="currentColor"
				stroke-width="2"
				fill="none"><path d="M15 3h6v6M9 21H3v-6M21 3l-7 7M3 21l7-7"></path></svg
			>
		</button>

		<button onclick={zoomOut}>-</button>
		<span style="white-space: nowrap; width: 40px; text-align: center;"
			>{Math.round(scale * 100)}%</span
		>
		<button onclick={zoomIn}>+</button>

		{#if showAttachButton && onAttachNote && !pdfCompact}
			<div
				class="divider"
				style="width: 1px; height: 16px; background: var(--border-default); margin: 0 4px;"
			></div>
			<button class="attach-note-btn" onclick={onAttachNote} title="Attach Note">
				<svg
					viewBox="0 0 24 24"
					width="14"
					height="14"
					stroke="currentColor"
					stroke-width="2"
					fill="none"
					><path d="M12 20h9"></path><path
						d="M16.5 3.5a2.121 2.121 0 0 1 3 3L7 19l-4 1 1-4L16.5 3.5z"
					></path></svg
				>
				<span style="margin-left: 4px;">Attach Note</span>
			</button>
		{/if}

		{#if pdfCompact}
			<PdfCompactMenu
				bind:toolMode
				bind:scrollMode
				bind:spreadMode
				bind:showMoreMenu
				{showAttachButton}
				{onAttachNote}
			/>
		{/if}

		{#if onClosePdf}
			<div
				class="divider"
				style="width: 1px; height: 16px; background: var(--border-default); margin: 0 4px;"
			></div>
			<button class="close-pdf-btn" onclick={onClosePdf} title="Close PDF" aria-label="Close PDF">
				<svg
					viewBox="0 0 24 24"
					width="14"
					height="14"
					stroke="currentColor"
					stroke-width="2"
					fill="none"
					stroke-linecap="round"
					stroke-linejoin="round"
					><line x1="18" y1="6" x2="6" y2="18"></line><line x1="6" y1="6" x2="18" y2="18"
					></line></svg
				>
			</button>
		{/if}
	</div>
