<script lang="ts">
	import { onMount, onDestroy, tick } from 'svelte';
	import { invoke } from '@tauri-apps/api/core';
	import * as pdfjsLib from 'pdfjs-dist';
	import pdfWorkerUrl from 'pdfjs-dist/build/pdf.worker.mjs?url';
	// PDF.js keeps the text-layer positioning rules in its viewer stylesheet.
	// Without this import the selectable spans render in normal document flow
	// below the canvas instead of overlaying the PDF page.
	import 'pdfjs-dist/web/pdf_viewer.css';
	import type { PdfAnnotation } from '$lib/types';
	import { pdfItemsToStructuralText, suppressRepeatedPageFurniture } from '$lib/extraction/structuralText';
	import PdfToolbar from '$lib/pdf/components/PdfToolbar.svelte';
	import PdfPages from '$lib/pdf/components/PdfPages.svelte';
	import PdfSelectionToolbar from '$lib/pdf/components/PdfSelectionToolbar.svelte';
	import '$lib/pdf/pdf-viewer.css';

	// The worker is loaded via Vite config or directly from dist
	pdfjsLib.GlobalWorkerOptions.workerSrc = pdfWorkerUrl;

	let {
		pdfBytes,
		annotations = [],
		onQuote,
		onAnnotationsChange,
		onImageExtract,
		onTextExtracted,
		onActiveSection,
		onSectionsReady,
		onAttachNote,
		onClosePdf,
		showAttachButton = true
	}: {
		pdfBytes: Uint8Array;
		annotations?: PdfAnnotation[];
		onQuote?: (
			quote: string,
			pageNum: number,
			rects?: { x: number; y: number; width: number; height: number }[]
		) => void;
		onAnnotationsChange?: (annots: PdfAnnotation[]) => void;
		onImageExtract?: (base64: string) => void;
		onTextExtracted?: (text: string) => void;
		onActiveSection?: (section: { key: string; label: string; content: string }) => void;
		onSectionsReady?: (
			sections: { key: string; label: string; content: string }[]
		) => void;
		onAttachNote?: () => void;
		onClosePdf?: () => void;
		showAttachButton?: boolean;
	} = $props();

	let containerDiv: HTMLDivElement | undefined = $state();
	let pdfViewerDiv: HTMLDivElement | undefined = $state();

	let pdfDoc: any = $state.raw(null);
	let numPages = $state(0);
	let scale = $state(1.2);
	let defaultViewport: any = $state.raw(null);

	// Layout & Scroll Modes
	type ScrollMode = 'vertical' | 'horizontal' | 'wrapped' | 'page';
	type SpreadMode = 'none' | 'odd' | 'even';

	let scrollMode = $state<ScrollMode>('page');
	let spreadMode = $state<SpreadMode>('none');
	let showDisplayMenu = $state(false);

	let selectionText = '';
	let showToolbar = $state(false);
	let toolbarStyle = $state('');
	let activeSelectionPage: number | null = null;

	// Tools State
	type ToolMode = 'select' | 'pen' | 'eraser' | 'marquee';
	let toolMode = $state<ToolMode>('select');
	let isDrawing = $state(false);
	let currentPath = $state<[number, number][]>([]);
	let currentRect = $state<[number, number, number, number] | null>(null);
	let activeDrawingPage = $state<number | null>(null);
	let selectionRects = $state<[number, number, number, number][]>([]);
	let errorMessage = $state('');
	let pdfControlsEl: HTMLElement | undefined = $state();
	let pdfCompact = $state(false);
	let showMoreMenu = $state(false);
	let loadGeneration = 0;
	let activePage = $state(0);
	let sectionReportGeneration = 0;
	let handleViewerScroll: (() => void) | null = null;

	async function extractDocumentText(doc: any, generation: number) {
		const extractedPages: string[] = [];
		for (let pageNumber = 1; pageNumber <= doc.numPages; pageNumber += 1) {
			if (generation !== loadGeneration) return;
			const page = await doc.getPage(pageNumber);
			const content = await page.getTextContent();
			let text = pdfItemsToStructuralText(content.items);
			extractedPages.push(text);
		}
		const pages = suppressRepeatedPageFurniture(extractedPages).map((text, index) => `[Page ${index + 1}]\n${text}`);
		if (generation === loadGeneration) {
			onTextExtracted?.(pages.join('\n\n').trim());
			// Eager section cache: every page becomes a cacheable section so the
			// model never re-evaluates a page the user jumps to. Active page
			// first (the reader is on it), then the rest in document order.
			const active = activePage || 1;
			const ordered = [
				active,
				...Array.from({ length: pages.length }, (_, i) => i + 1).filter(
					(page) => page !== active
				)
			];
			onSectionsReady?.(
				ordered.map((pageNumber) => ({
					key: `page:${pageNumber}`,
					label: `Page ${pageNumber}`,
					content: pages[pageNumber - 1]
				}))
			);
		}
	}

	async function reportPageSection(pageNumber: number) {
		if (!pdfDoc || !onActiveSection || pageNumber < 1 || pageNumber > numPages) return;
		const generation = ++sectionReportGeneration;
		try {
			const page = await pdfDoc.getPage(pageNumber);
			const content = await page.getTextContent();
			const text = pdfItemsToStructuralText(content.items);
			if (generation === sectionReportGeneration && text) {
				onActiveSection({
					key: `page:${pageNumber}`,
					label: `Page ${pageNumber}`,
					content: `[Page ${pageNumber}]\n${text}`
				});
			}
		} catch (error) {
			console.debug('Could not read active PDF page:', error);
		}
	}

	function changePage(delta: number) {
		if (!numPages) return;
		const nextPage = Math.min(numPages, Math.max(1, (activePage || 1) + delta));
		if (nextPage === activePage) return;
		activePage = nextPage;
		void reportPageSection(nextPage);
	}

	function reportActiveSection() {
		if (!pdfViewerDiv || !pdfDoc || !onActiveSection) return;
		const root = pdfViewerDiv.getBoundingClientRect();
		let bestPage = 0;
		let bestVisible = 0;
		for (const element of Array.from(pdfViewerDiv.querySelectorAll<HTMLElement>('.pdf-page-container'))) {
			const page = Number(element.dataset.pageNumber);
			const rect = element.getBoundingClientRect();
			const visible = Math.max(0, Math.min(rect.bottom, root.bottom) - Math.max(rect.top, root.top));
			if (visible > bestVisible) {
				bestVisible = visible;
				bestPage = page;
			}
		}
		if (!bestPage || bestPage === activePage) return;
		activePage = bestPage;
		void reportPageSection(bestPage);
	}

	async function loadPdf() {
		const generation = ++loadGeneration;
		try {
			errorMessage = '';
			// PDF.js enables document scripts through its viewer-layer
			// PDFScriptingManager, not through getDocument. This custom reader uses
			// only the display and text APIs and deliberately never creates that
			// manager or an annotation layer (equivalent to enableScripting: false).
			const loadingTask = pdfjsLib.getDocument({ data: pdfBytes });
			const doc = await loadingTask.promise;

			const page1 = await doc.getPage(1);
			defaultViewport = page1.getViewport({ scale: 1.0 });

			pdfDoc = doc;
			numPages = doc.numPages;
			activePage = 1;
			void extractDocumentText(doc, generation);

			// Fit to the pane width as soon as the PDF renders, so it isn't cropped
			// on narrow/split windows. Wait for the viewer div to mount + lay out.
			await tick();
			requestAnimationFrame(() => {
				fitToScreen();
				void reportPageSection(activePage || 1);
			});
		} catch (error: any) {
			console.error('Error loading PDF:', error);
			errorMessage = error?.message || String(error);
		}
	}

	function fitToScreen() {
		if (!defaultViewport || !pdfViewerDiv) return;
		// Reserve space for the 2rem container padding AND a vertical scrollbar that
		// appears once tall content renders — otherwise the page is a hair too wide
		// and leaves a stray horizontal scrollbar.
		const SCROLLBAR = 18;
		if (spreadMode !== 'none') {
			const targetWidth = pdfViewerDiv.clientWidth - 96 - SCROLLBAR;
			if (targetWidth > 0) scale = targetWidth / (defaultViewport.width * 2);
		} else {
			const targetWidth = pdfViewerDiv.clientWidth - 64 - SCROLLBAR;
			if (targetWidth > 0) scale = targetWidth / defaultViewport.width;
		}
	}

	function zoomIn() {
		scale += 0.2;
	}
	function zoomOut() {
		if (scale > 0.4) scale -= 0.2;
	}

	// Drawing Handlers
	function handlePointerDown(e: PointerEvent, pageNum: number, currentScale: number) {
		if (toolMode === 'select') return;
		const target = e.currentTarget as HTMLElement;
		target.setPointerCapture(e.pointerId);
		isDrawing = true;
		activeDrawingPage = pageNum;

		const rect = target.getBoundingClientRect();
		const x = (e.clientX - rect.left) / currentScale;
		const y = (e.clientY - rect.top) / currentScale;

		if (toolMode === 'pen') {
			currentPath = [[x, y]];
		} else if (toolMode === 'marquee') {
			currentRect = [x, y, 0, 0];
		} else if (toolMode === 'eraser') {
			eraseAt([x, y], pageNum, currentScale);
		}
	}

	function handlePointerMove(e: PointerEvent, currentScale: number) {
		if (!isDrawing || toolMode === 'select' || !activeDrawingPage) return;
		const target = e.currentTarget as HTMLElement;
		const rect = target.getBoundingClientRect();
		const x = (e.clientX - rect.left) / currentScale;
		const y = (e.clientY - rect.top) / currentScale;

		if (toolMode === 'pen') {
			currentPath = [...currentPath, [x, y]];
		} else if (toolMode === 'marquee' && currentRect) {
			const startX = currentRect[0];
			const startY = currentRect[1];
			currentRect = [startX, startY, x - startX, y - startY];
		} else if (toolMode === 'eraser') {
			eraseAt([x, y], activeDrawingPage, currentScale);
		}
	}

	function handlePointerUp(
		e: PointerEvent,
		pageNum: number,
		currentScale: number,
		canvasEl: HTMLCanvasElement
	) {
		if (!isDrawing || toolMode === 'select') return;
		const target = e.currentTarget as HTMLElement;
		target.releasePointerCapture(e.pointerId);
		isDrawing = false;

		if (toolMode === 'pen' && currentPath.length > 1 && activeDrawingPage === pageNum) {
			const newAnn: PdfAnnotation = {
				id: crypto.randomUUID(),
				page: pageNum,
				type: 'draw',
				points: currentPath,
				color: '#ef4444',
				strokeWidth: 2
			};
			if (onAnnotationsChange) onAnnotationsChange([...annotations, newAnn]);
		} else if (
			toolMode === 'marquee' &&
			currentRect &&
			onImageExtract &&
			canvasEl &&
			activeDrawingPage === pageNum
		) {
			const [x, y, w, h] = currentRect;
			const realX = Math.min(x, x + w) * currentScale;
			const realY = Math.min(y, y + h) * currentScale;
			const realW = Math.abs(w) * currentScale;
			const realH = Math.abs(h) * currentScale;

			if (realW > 10 && realH > 10) {
				const cropCanvas = document.createElement('canvas');
				cropCanvas.width = realW;
				cropCanvas.height = realH;
				const ctx = cropCanvas.getContext('2d');
				if (ctx) {
					ctx.drawImage(canvasEl, realX, realY, realW, realH, 0, 0, realW, realH);
					const base64 = cropCanvas.toDataURL('image/png');
					onImageExtract(base64);
				}
			}
		}

		currentPath = [];
		currentRect = null;
		activeDrawingPage = null;
	}

	function eraseAt([x, y]: [number, number], pageNum: number, currentScale: number) {
		const ERASER_RADIUS = 15 / currentScale;
		let modified = false;
		const filtered = annotations.filter((ann) => {
			if (ann.page !== pageNum || !ann.points) return true;
			const hit = ann.points.some((p) => {
				const dx = p[0] - x;
				const dy = p[1] - y;
				return Math.sqrt(dx * dx + dy * dy) < ERASER_RADIUS;
			});
			if (hit) modified = true;
			return !hit;
		});

		if (modified && onAnnotationsChange) {
			onAnnotationsChange(filtered);
		}
	}

	// Text Selection Handlers
	let selectionTimeout: ReturnType<typeof setTimeout> | undefined;
	let handleDocumentClick: ((e: MouseEvent) => void) | null = null;

	function handleSelection() {
		if (toolMode !== 'select') return;
		const selection = window.getSelection();

		if (!selection || selection.isCollapsed) {
			if (selectionTimeout) clearTimeout(selectionTimeout);
			showToolbar = false;
			return;
		}

		const anchorNode = selection.anchorNode;
		if (!anchorNode) return;

		const textLayerDiv = anchorNode.parentElement?.closest('.textLayer');
		if (textLayerDiv && pdfViewerDiv) {
			if (selectionTimeout) clearTimeout(selectionTimeout);

			selectionTimeout = setTimeout(() => {
				const currentSelection = window.getSelection();
				if (!currentSelection || currentSelection.isCollapsed) return;

				selectionText = currentSelection.toString().trim();
				if (selectionText) {
					const range = currentSelection.getRangeAt(0);
					const rects = Array.from(range.getClientRects());
					const pageContainer = textLayerDiv.closest('.pdf-page-container');

					if (pageContainer) {
						const pageNumAttr = pageContainer.getAttribute('data-page-number');
						if (pageNumAttr) {
							activeSelectionPage = parseInt(pageNumAttr);
						}

						const svgLayer = pageContainer.querySelector('.annotation-layer');
						if (svgLayer && pdfViewerDiv) {
							const viewerRect = pdfViewerDiv.getBoundingClientRect();
							const svgRect = svgLayer.getBoundingClientRect();

							const rect = range.getBoundingClientRect();
							const top = rect.top - viewerRect.top + pdfViewerDiv.scrollTop - 40;
							const left =
								rect.left - viewerRect.left + pdfViewerDiv.scrollLeft + rect.width / 2 - 100;

							toolbarStyle = `top: ${top}px; left: ${left}px;`;

							selectionRects = rects.map((r) => {
								const svgEl = svgLayer as unknown as SVGSVGElement;
								const vb = svgEl.viewBox.baseVal;
								const realScaleX = svgRect.width / vb.width;
								const realScaleY = svgRect.height / vb.height;

								return [
									(r.left - svgRect.left) / realScaleX,
									(r.top - svgRect.top) / realScaleY,
									r.width / realScaleX,
									r.height / realScaleY
								];
							});
							showToolbar = true;
							return;
						}
					}
				}
				showToolbar = false;
			}, 350);
		} else {
			showToolbar = false;
		}
	}

	function handleQuote() {
		if (selectionText && onQuote && activeSelectionPage !== null) {
			onQuote(selectionText, activeSelectionPage);
			window.getSelection()?.removeAllRanges();
			showToolbar = false;
		}
	}

	function applyHighlight(color: string) {
		if (selectionText && selectionRects.length > 0 && activeSelectionPage !== null) {
			const newAnn: PdfAnnotation = {
				id: crypto.randomUUID(),
				page: activeSelectionPage,
				type: 'text_highlight',
				rects: selectionRects,
				color: color,
				strokeWidth: 0
			};
			if (onAnnotationsChange) onAnnotationsChange([...annotations, newAnn]);
			window.getSelection()?.removeAllRanges();
			showToolbar = false;
		}
	}

	async function handleCopy() {
		if (selectionText) {
			try {
				await navigator.clipboard.writeText(selectionText);
			} catch (e) {
				console.error('Failed to copy:', e);
			}
			window.getSelection()?.removeAllRanges();
			showToolbar = false;
		}
	}

	$effect(() => {
		if (pdfBytes) {
			loadPdf();
		}
	});

	$effect(() => {
		if (!pdfControlsEl) return;
		const observer = new ResizeObserver(() => {
			pdfCompact = (pdfControlsEl?.clientWidth ?? 999) < 520;
		});
		observer.observe(pdfControlsEl);
		return () => observer.disconnect();
	});

	onMount(() => {
		handleViewerScroll = () => requestAnimationFrame(() => void reportActiveSection());
		pdfViewerDiv?.addEventListener('scroll', handleViewerScroll, { passive: true });
		document.addEventListener('selectionchange', handleSelection);
		handleDocumentClick = (e: MouseEvent) => {
			const target = e.target as HTMLElement;
			if (!target.closest('.display-menu-container')) {
				showDisplayMenu = false;
			}
			if (!target.closest('.more-menu-container')) {
				showMoreMenu = false;
			}
		};
		document.addEventListener('click', handleDocumentClick);
	});

	onDestroy(() => {
		if (handleViewerScroll) pdfViewerDiv?.removeEventListener('scroll', handleViewerScroll);
		document.removeEventListener('selectionchange', handleSelection);
		if (handleDocumentClick) {
			document.removeEventListener('click', handleDocumentClick);
		}
	});
</script>
<div class="pdf-wrapper" bind:this={containerDiv}>
	<PdfToolbar
		bind:toolMode
		bind:scrollMode
		bind:spreadMode
		bind:showDisplayMenu
		bind:showMoreMenu
		bind:pdfCompact
		bind:activePage
		bind:numPages
		bind:scale
		bind:pdfControlsEl
		{showAttachButton}
		{onAttachNote}
		{onClosePdf}
		{fitToScreen}
		{changePage}
		{zoomIn}
		{zoomOut}
	/>
	<div class="pdf-viewer-scroll-area" bind:this={pdfViewerDiv}>
		{#if errorMessage}
			<div
					style="color: var(--danger-text); padding: 2rem; background: var(--danger-bg); border-radius: var(--radius-lg); margin: 2rem;"
			>
				<h3 style="margin-top: 0;">Error Loading PDF</h3>
				<pre style="white-space: pre-wrap; font-family: monospace;">{errorMessage}</pre>
			</div>
		{/if}
 
		<PdfPages
			{pdfDoc}
			{defaultViewport}
			{numPages}
			{activePage}
			{scrollMode}
			{spreadMode}
			{scale}
			{annotations}
			{toolMode}
			{isDrawing}
			{activeDrawingPage}
			{currentPath}
			{currentRect}
			{onAnnotationsChange}
			{onImageExtract}
			onPointerDown={handlePointerDown}
			onPointerMove={handlePointerMove}
			onPointerUp={handlePointerUp}
			{pdfViewerDiv}
		/>
		<PdfSelectionToolbar
			{showToolbar}
			{toolMode}
			{toolbarStyle}
			{applyHighlight}
			{handleCopy}
			{handleQuote}
		/>
	</div>
</div>
