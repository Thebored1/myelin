<script lang="ts">
	import { onMount } from 'svelte';
	import {
		buildTextIndex,
		inkHit,
		normalizedRect,
		rectsForRange,
		serializeSelection
	} from './model';
	import { renderSource } from './renderers';
	import '$lib/pdf/pdf-viewer.css';
	import type { PaneTextIndex, SourceAnnotation, SourceKind, SourceToolMode } from './types';

	let {
		kind,
		bytes,
		annotations = [],
		onAnnotationsChange,
		onTextExtracted,
		onAttachNote,
		onClosePdf,
		showAttachButton = true
	}: {
		kind: SourceKind;
		bytes: Uint8Array;
		annotations?: SourceAnnotation[];
		onAnnotationsChange?: (annotations: SourceAnnotation[]) => void;
		onTextExtracted?: (text: string) => void;
		onAttachNote?: () => void;
		onClosePdf?: () => void;
		showAttachButton?: boolean;
	} = $props();

	const INK_COLORS = ['#E05555', '#EAB308', '#4CAF50', '#64B5F6'];
	const HIGHLIGHT_COLORS = [
		'rgba(253, 224, 71, 0.4)',
		'rgba(129, 199, 132, 0.4)',
		'rgba(100, 181, 246, 0.4)',
		'rgba(239, 111, 111, 0.4)'
	];

	let content: HTMLElement | undefined = $state();
	let overlay: SVGSVGElement | undefined = $state();
	let toolMode = $state<SourceToolMode>('select');
	let inkColor = $state(INK_COLORS[0]);
	let marqueeRect = $state<{ x: number; y: number; width: number; height: number } | null>(null);
	let selectionToolbar = $state<{ x: number; y: number } | null>(null);
	let selectionAnchor = $state<{ start: number; end: number; quote: string } | null>(null);
	let textIndex = $state<PaneTextIndex>({ segments: [], length: 0 });
	let overlaySize = $state({ width: 0, height: 0 });
	let highlightRects = $state<
		{
			annotation: SourceAnnotation;
			rects: { x: number; y: number; width: number; height: number }[];
		}[]
	>([]);

	let drawing: { points: [number, number][] } | null = $state(null);
	let marqueeStart: [number, number] | null = null;

	function commit(next: SourceAnnotation[]) {
		onAnnotationsChange?.(next);
	}

	function overlayPoint(event: MouseEvent): [number, number] {
		if (!content) return [0, 0];
		const origin = content.getBoundingClientRect();
		const width = content.scrollWidth || origin.width;
		const height = content.scrollHeight || origin.height;
		return [
			Math.max(0, Math.min(1, (event.clientX - origin.left + content.scrollLeft) / width)),
			Math.max(0, Math.min(1, (event.clientY - origin.top + content.scrollTop) / height))
		];
	}

	function toOverlay(point: [number, number]): [number, number] {
		return [point[0] * overlaySize.width, point[1] * overlaySize.height];
	}

	function redraw() {
		if (!content) return;
		overlaySize = {
			width: Math.max(content.scrollWidth, content.clientWidth),
			height: Math.max(content.scrollHeight, content.clientHeight)
		};
		textIndex = buildTextIndex(content);
		highlightRects = annotations
			.filter(
				(annotation): annotation is Extract<SourceAnnotation, { kind: 'highlight' }> =>
					annotation.kind === 'highlight'
			)
			.map((annotation) => ({
				annotation,
				rects: rectsForRange(textIndex, annotation.start, annotation.end, content as HTMLElement)
			}))
			.filter((entry) => entry.rects.length > 0);
	}

	let docxNaturalWidth = 0;

	// Scale true-size Word pages down to the pane width (print-preview style).
	function fitDocx() {
		if (kind !== 'docx' || !content) return;
		const wrapper = content.querySelector('.docx-wrapper') as HTMLElement | null;
		const section = content.querySelector('section.docx') as HTMLElement | null;
		if (!wrapper || !section) return;
		if (docxNaturalWidth === 0) docxNaturalWidth = section.offsetWidth || 0;
		if (!docxNaturalWidth) return;
		const available = Math.max(120, content.clientWidth - 24);
		wrapper.style.zoom = String(Math.max(0.3, Math.min(1.25, available / docxNaturalWidth)));
	}

	onMount(() => {
		if (!content) return;
		let getText = () => content?.innerText ?? '';
		void renderSource(kind, bytes, content).then((rendered) => {
			getText = rendered.getText;
			fitDocx();
			redraw();
			if (onTextExtracted) onTextExtracted(getText());
		});
		const observer = new ResizeObserver(() => {
			fitDocx();
			redraw();
		});
		observer.observe(content);
		content.addEventListener('scroll', redraw, { passive: true });
		return () => observer.disconnect();
	});

	function onOverlayDown(event: MouseEvent) {
		if (toolMode === 'select') return;
		event.preventDefault();
		const point = overlayPoint(event);
		if (toolMode === 'pen') {
			drawing = { points: [point] };
		} else if (toolMode === 'marquee') {
			marqueeStart = point;
			marqueeRect = null;
		} else if (toolMode === 'eraser') {
			const radius = 0.012;
			const scaled: [number, number] = [
				point[0] * overlaySize.width,
				point[1] * overlaySize.height
			];
			const next = annotations.filter((annotation) => {
				if (annotation.kind !== 'ink') return true;
				const path = annotation.points.map(
					(p) => [p[0] * overlaySize.width, p[1] * overlaySize.height] as [number, number]
				);
				return !inkHit(scaled, path, radius * overlaySize.width);
			});
			if (next.length !== annotations.length) commit(next);
		}
	}

	function onOverlayMove(event: MouseEvent) {
		if (toolMode === 'pen' && drawing) {
			drawing = { points: [...drawing.points, overlayPoint(event)] };
		} else if (toolMode === 'marquee' && marqueeStart) {
			marqueeRect = normalizedRect(
				...toOverlay(marqueeStart),
				...toOverlay(overlayPoint(event))
			) as { x: number; y: number; width: number; height: number };
		}
	}

	function onOverlayUp() {
		if (toolMode === 'pen' && drawing && drawing.points.length > 1) {
			commit([
				...annotations,
				{
					id: crypto.randomUUID(),
					kind: 'ink',
					color: inkColor,
					strokeWidth: 2.5,
					points: drawing.points
				}
			]);
		}
		drawing = null;
		if (toolMode === 'marquee') {
			marqueeStart = null;
			setTimeout(() => (marqueeRect = null), 200);
		}
	}

	function onContentMouseUp() {
		if (toolMode !== 'select' || !content) return;
		const current = window.getSelection();
		if (!current) return;
		const anchor = serializeSelection(content, current);
		if (!anchor) {
			selectionToolbar = null;
			selectionAnchor = null;
			return;
		}
		const origin = content.getBoundingClientRect();
		const selection = window.getSelection();
		const rect =
			selection && selection.rangeCount > 0
				? selection.getRangeAt(0).getBoundingClientRect()
				: origin;
		selectionAnchor = anchor;
		selectionToolbar = {
			x: Math.max(8, rect.left - origin.left + rect.width / 2 - 70),
			y: rect.top - origin.top - 44
		};
	}

	function applyHighlight(color: string) {
		if (!selectionAnchor) return;
		commit([
			...annotations,
			{
				id: crypto.randomUUID(),
				kind: 'highlight',
				color,
				start: selectionAnchor.start,
				end: selectionAnchor.end,
				quote: selectionAnchor.quote
			}
		]);
		selectionAnchor = null;
		selectionToolbar = null;
		window.getSelection()?.removeAllRanges();
	}

	function copySelection() {
		void navigator.clipboard.writeText(selectionAnchor?.quote ?? '');
		selectionAnchor = null;
		selectionToolbar = null;
	}

	function clearAll() {
		commit([]);
	}
</script>

<div class="source-pane">
	<div class="pdf-controls">
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
			{#each INK_COLORS as color (color)}
				<button
					class="swatch-btn"
					class:active={inkColor === color}
					style="--swatch-color: {color}; --swatch-border: {color};"
					onclick={() => {
						inkColor = color;
						toolMode = 'pen';
					}}
					aria-label="Ink color {color}"><span class="swatch-icon">●</span></button
				>
			{/each}
			<button onclick={clearAll} title="Clear all annotations">
				<svg
					viewBox="0 0 24 24"
					width="14"
					height="14"
					stroke="currentColor"
					stroke-width="2"
					fill="none"
					><path
						d="M3 6h18M8 6V4a1 1 0 0 1 1-1h6a1 1 0 0 1 1 1v2m3 0v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6"
					></path></svg
				>
			</button>
		</div>

		<div style="flex-grow: 1;"></div>

		{#if showAttachButton && onAttachNote}
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

		{#if onClosePdf}
			<div
				class="divider"
				style="width: 1px; height: 16px; background: var(--border-default); margin: 0 4px;"
			></div>
			<button
				class="close-pdf-btn"
				onclick={onClosePdf}
				title="Close source"
				aria-label="Close source"
			>
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
	<div class="source-body">
		<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
		<div
			class="source-content"
			bind:this={content}
			class:nos-select={toolMode !== 'select'}
			role="document"
			onmouseup={() => onContentMouseUp()}
		></div>
		<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
		<svg
			bind:this={overlay}
			class="source-overlay"
			class:interactive={toolMode !== 'select'}
			width={overlaySize.width}
			height={overlaySize.height}
			viewBox="0 0 {overlaySize.width} {overlaySize.height}"
			role="application"
			aria-label="Annotation overlay"
			onmousedown={onOverlayDown}
			onmousemove={onOverlayMove}
			onmouseup={onOverlayUp}
		>
			{#each highlightRects as entry (entry.annotation.id)}
				{#each entry.rects as rect, rectIndex (rectIndex)}
					<rect
						x={rect.x}
						y={rect.y}
						width={rect.width}
						height={rect.height}
						fill={entry.annotation.color}
						rx="2"
					></rect>
				{/each}
			{/each}
			{#each annotations as annotation (annotation.id)}
				{#if annotation.kind === 'ink'}
					<polyline
						points={annotation.points.map((point) => toOverlay(point).join(',')).join(' ')}
						fill="none"
						stroke={annotation.color}
						stroke-width={annotation.strokeWidth}
						stroke-linecap="round"
						stroke-linejoin="round"
					></polyline>
				{/if}
			{/each}
			{#if drawing}
				<polyline
					points={drawing.points.map((point) => toOverlay(point).join(',')).join(' ')}
					fill="none"
					stroke={inkColor}
					stroke-width="2.5"
					stroke-linecap="round"
					stroke-linejoin="round"
				></polyline>
			{/if}
			{#if marqueeRect}
				<rect
					x={marqueeRect.x}
					y={marqueeRect.y}
					width={marqueeRect.width}
					height={marqueeRect.height}
					fill="rgba(100, 181, 246, 0.12)"
					stroke="var(--accent-100, #64B5F6)"
					stroke-dasharray="4 3"
				></rect>
			{/if}
		</svg>
	</div>
	{#if selectionToolbar}
		<div
			class="floating-toolbar"
			style="left: {selectionToolbar.x}px; top: {selectionToolbar.y}px;"
		>
			{#each HIGHLIGHT_COLORS as color (color)}
				<button
					class="swatch-btn"
					style="--swatch-color: {color}; --swatch-border: #eab308;"
					onclick={() => applyHighlight(color)}
					aria-label="Highlight {color}"
				></button>
			{/each}
			<button class="swatch-btn" onclick={copySelection} aria-label="Copy" title="Copy"
				><span class="swatch-icon">⧉</span></button
			>
		</div>
	{/if}
</div>

<style>
	.source-pane {
		position: relative;
		display: flex;
		flex-direction: column;
		min-width: 0;
		height: 100%;
		overflow: hidden;
	}
	.source-content {
		position: relative;
		flex: 1;
		min-height: 0;
		overflow: auto;
		padding: 1rem 1.25rem;
		background: var(--bg-page);
		color: var(--text-primary);
		font-size: 0.95rem;
		line-height: 1.6;
	}
	.source-content :global(h1) {
		font-size: 1.65rem;
		font-weight: 650;
		line-height: 1.25;
		margin: 1.6em 0 0.5em;
	}
	.source-content :global(h2) {
		font-size: 1.35rem;
		font-weight: 620;
		line-height: 1.3;
		margin: 1.4em 0 0.45em;
	}
	.source-content :global(h3) {
		font-size: 1.12rem;
		font-weight: 600;
		margin: 1.2em 0 0.4em;
	}
	.source-content :global(h4),
	.source-content :global(h5),
	.source-content :global(h6) {
		font-size: 0.98rem;
		font-weight: 600;
		margin: 1em 0 0.35em;
	}
	.source-content :global(p) {
		margin: 0 0 0.85em;
	}
	.source-content :global(ul),
	.source-content :global(ol) {
		margin: 0 0 0.85em;
		padding-left: 1.5em;
	}
	.source-content :global(li) {
		margin-bottom: 0.3em;
	}
	.source-content :global(blockquote) {
		margin: 0 0 0.85em;
		padding: 0.2em 0 0.2em 0.9em;
		border-left: 3px solid var(--border-strong);
		color: var(--text-secondary);
	}
	.source-content :global(hr) {
		border: none;
		border-top: 1px solid var(--border-default);
		margin: 1.6em 0;
	}
	.source-content :global(img) {
		max-width: 100%;
		height: auto;
		border-radius: var(--radius-md);
	}
	.source-content :global(a) {
		color: var(--accent-100);
		text-decoration: underline;
		text-underline-offset: 2px;
	}
	.source-content :global(strong) {
		font-weight: 620;
	}
	.source-content :global(table) {
		border-collapse: collapse;
		margin: 0 0 1em;
		width: 100%;
	}
	.source-content :global(td),
	.source-content :global(th) {
		border: 1px solid var(--border-default);
		padding: 5px 9px;
		text-align: left;
	}
	.source-content :global(th) {
		background: var(--bg-elevated);
		font-weight: 600;
	}
	/* Reading width: long documents stay comfortable on wide panes. */
	.source-content :global(> *:not(.docx-wrapper)) {
		max-width: 46rem;
	}
	/* docx-preview renders true Word pages (sectPr geometry, margins, fonts).
	   The wrapper is zoomed to fit the pane width; pages keep their look. */
	.source-content :global(.docx-wrapper) {
		background: transparent;
		padding: 8px 12px;
		min-width: max-content;
	}
	.source-content :global(.docx-wrapper > section.docx) {
		box-shadow: 0 1px 6px var(--shadow-color-strong);
	}
	.source-content :global(.source-plain) {
		white-space: pre-wrap;
		word-break: break-word;
		font-family: var(--font-mono);
		font-size: 0.85rem;
		line-height: 1.55;
	}
	.source-content.nos-select {
		user-select: none;
	}
	.source-body {
		position: relative;
		flex: 1;
		min-height: 0;
		display: flex;
	}
	.source-overlay {
		position: absolute;
		top: 0;
		left: 0;
		pointer-events: none;
		z-index: 10;
	}
	.source-overlay.interactive {
		pointer-events: auto;
		cursor: crosshair;
	}
</style>
