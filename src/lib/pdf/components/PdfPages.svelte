<script lang="ts">
	import type { PdfAnnotation } from '$lib/types';
	import type { ScrollMode, SpreadMode, ToolMode } from '../types';
	import type { PDFDocumentProxy, PageViewport } from 'pdfjs-dist';
	import PdfPage from '$lib/components/PdfPage.svelte';

	let {
		pdfDoc,
		defaultViewport,
		numPages,
		activePage,
		scrollMode,
		spreadMode,
		scale,
		annotations = [],
		toolMode,
		isDrawing,
		activeDrawingPage,
		currentPath,
		currentRect,
		onAnnotationsChange,
		onImageExtract,
		onPointerDown,
		onPointerMove,
		onPointerUp,
		pdfViewerDiv
	}: {
		pdfDoc: PDFDocumentProxy | null;
		defaultViewport: PageViewport | null;
		numPages: number;
		activePage: number;
		scrollMode: ScrollMode;
		spreadMode: SpreadMode;
		scale: number;
		annotations: PdfAnnotation[];
		toolMode: ToolMode;
		isDrawing: boolean;
		activeDrawingPage: number | null;
		currentPath: [number, number][];
		currentRect: [number, number, number, number] | null;
		onAnnotationsChange?: (annots: PdfAnnotation[]) => void;
		onImageExtract?: (base64: string) => void;
		onPointerDown: (event: PointerEvent, page: number, scale: number) => void;
		onPointerMove: (event: PointerEvent, scale: number) => void;
		onPointerUp: (
			event: PointerEvent,
			page: number,
			scale: number,
			canvas: HTMLCanvasElement
		) => void;
		pdfViewerDiv?: HTMLDivElement;
	} = $props();
</script>

{#if pdfDoc && defaultViewport}
	<div class="pdf-pages-container layout-{scrollMode} spread-{spreadMode}">
		{#if scrollMode === 'page'}
			{#key activePage}
				<PdfPage
					{pdfDoc}
					pageNum={activePage || 1}
					{scale}
					{annotations}
					{toolMode}
					{isDrawing}
					currentPath={activeDrawingPage === activePage ? currentPath : []}
					currentRect={activeDrawingPage === activePage ? currentRect : null}
					{onAnnotationsChange}
					{onImageExtract}
					{onPointerDown}
					{onPointerMove}
					{onPointerUp}
					{pdfViewerDiv}
					{defaultViewport}
				/>
			{/key}
		{:else}
			{#each Array(numPages) as page, i (String(page) + i)}
				{@const pageNum = i + 1}
				<PdfPage
					{pdfDoc}
					{pageNum}
					{scale}
					{annotations}
					{toolMode}
					{isDrawing}
					currentPath={activeDrawingPage === pageNum ? currentPath : []}
					currentRect={activeDrawingPage === pageNum ? currentRect : null}
					{onAnnotationsChange}
					{onImageExtract}
					{onPointerDown}
					{onPointerMove}
					{onPointerUp}
					{pdfViewerDiv}
					{defaultViewport}
				/>
			{/each}
		{/if}
	</div>
{/if}
