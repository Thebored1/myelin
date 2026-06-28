<script lang="ts">
	import { onMount, onDestroy, tick } from 'svelte';
	import * as pdfjsLib from 'pdfjs-dist';
	import pdfWorkerUrl from 'pdfjs-dist/build/pdf.worker.mjs?url';
	import 'pdfjs-dist/web/pdf_viewer.css';
	import type { PdfAnnotation } from '$lib/types';
	import PdfPage from './PdfPage.svelte';

	// The worker is loaded via Vite config or directly from dist
	pdfjsLib.GlobalWorkerOptions.workerSrc = pdfWorkerUrl;

	let { 
		pdfBytes, 
		annotations = [], 
		onQuote, 
		onAnnotationsChange,
		onImageExtract,
		onAttachNote,
		onClosePdf,
		showAttachButton = true
	}: { 
		pdfBytes: Uint8Array, 
		annotations?: PdfAnnotation[], 
		onQuote?: (quote: string, pageNum: number, rects?: {x:number, y:number, width:number, height:number}[]) => void,
		onAnnotationsChange?: (annots: PdfAnnotation[]) => void,
		onImageExtract?: (base64: string) => void,
		onAttachNote?: () => void,
		onClosePdf?: () => void,
		showAttachButton?: boolean
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
	
	let scrollMode = $state<ScrollMode>('vertical');
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

	async function loadPdf() {
		try {
			errorMessage = '';
			const loadingTask = pdfjsLib.getDocument({ data: pdfBytes });
			const doc = await loadingTask.promise;
			
			const page1 = await doc.getPage(1);
			defaultViewport = page1.getViewport({ scale: 1.0 });
			
			pdfDoc = doc;
			numPages = doc.numPages;

			// Fit to the pane width as soon as the PDF renders, so it isn't cropped
			// on narrow/split windows. Wait for the viewer div to mount + lay out.
			await tick();
			requestAnimationFrame(() => fitToScreen());
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

	function zoomIn() { scale += 0.2; }
	function zoomOut() { if (scale > 0.4) scale -= 0.2; }

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

	function handlePointerUp(e: PointerEvent, pageNum: number, currentScale: number, canvasEl: HTMLCanvasElement) {
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
		} else if (toolMode === 'marquee' && currentRect && onImageExtract && canvasEl && activeDrawingPage === pageNum) {
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
		const filtered = annotations.filter(ann => {
			if (ann.page !== pageNum || !ann.points) return true;
			const hit = ann.points.some(p => {
				const dx = p[0] - x;
				const dy = p[1] - y;
				return Math.sqrt(dx*dx + dy*dy) < ERASER_RADIUS;
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
							const left = rect.left - viewerRect.left + pdfViewerDiv.scrollLeft + (rect.width / 2) - 100;

							toolbarStyle = `top: ${top}px; left: ${left}px;`;
							
							selectionRects = rects.map(r => {
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
				console.error("Failed to copy:", e);
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
		document.removeEventListener('selectionchange', handleSelection);
		if (handleDocumentClick) {
			document.removeEventListener('click', handleDocumentClick);
		}
	});
</script>

<div class="pdf-wrapper" bind:this={containerDiv}>
	<div class="pdf-controls" bind:this={pdfControlsEl}>
		{#if !pdfCompact}
			<!-- Annotation Tools -->
			<div class="tools-group">
				<button class:active={toolMode === 'select'} onclick={() => toolMode = 'select'} title="Select Text">
					<svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none"><path d="M3 3l7.07 16.97 2.51-7.39 7.39-2.51L3 3z"></path><path d="M13 13l6 6"></path></svg>
				</button>
				<button class:active={toolMode === 'pen'} onclick={() => toolMode = 'pen'} title="Draw">
					<svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none"><path d="M12 19l7-7 3 3-7 7-3-3z"></path><path d="M18 13l-1.5-7.5L2 2l3.5 14.5L13 18l5-5z"></path><path d="M2 2l7.586 7.586"></path><circle cx="11" cy="11" r="2"></circle></svg>
				</button>
				<button class:active={toolMode === 'eraser'} onclick={() => toolMode = 'eraser'} title="Eraser">
					<svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none"><path d="M20 20H7L3 16C2.5 15.5 2.5 14.5 3 14L13 4C13.5 3.5 14.5 3.5 15 4L20 9C20.5 9.5 20.5 10.5 20 11L11 20H20"></path></svg>
				</button>
				<button class:active={toolMode === 'marquee'} onclick={() => toolMode = 'marquee'} title="Marquee / Crop">
					<svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none"><rect x="3" y="3" width="18" height="18" rx="2" ry="2" stroke-dasharray="4 4"></rect></svg>
				</button>
			</div>

			<div style="flex-grow: 1;"></div>

			<span style="white-space: nowrap;">{numPages} Pages</span>

			<div class="display-menu-container">
				<button class="display-btn" onclick={(e) => { e.stopPropagation(); showDisplayMenu = !showDisplayMenu; }} title="Layout Options">
					<svg viewBox="0 0 24 24" width="16" height="16" stroke="currentColor" stroke-width="2" fill="none" style="margin-right: 4px;"><rect x="4" y="4" width="16" height="16" rx="2" ry="2"></rect><line x1="4" y1="12" x2="20" y2="12"></line></svg>
					Layout
				</button>
				{#if showDisplayMenu}
					<div class="display-menu dropdown">
						<div class="menu-group">
							<button class:active={scrollMode === 'page' && spreadMode === 'none'} onclick={() => { scrollMode = 'page'; spreadMode = 'none'; showDisplayMenu = false; }}>
								<svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none"><rect x="7" y="3" width="10" height="18" rx="2"></rect><path d="M7 8h10M7 16h10"></path></svg>
								Page Scrolling
							</button>
							<button class:active={scrollMode === 'vertical'} onclick={() => { scrollMode = 'vertical'; showDisplayMenu = false; }}>
								<svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none"><rect x="7" y="3" width="10" height="18" rx="2"></rect><path d="M12 3v18"></path></svg>
								Vertical Scrolling
							</button>
							<button class:active={scrollMode === 'horizontal'} onclick={() => { scrollMode = 'horizontal'; showDisplayMenu = false; }}>
								<svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none"><rect x="3" y="7" width="18" height="10" rx="2"></rect><path d="M3 12h18"></path></svg>
								Horizontal Scrolling
							</button>
							<button class:active={scrollMode === 'wrapped'} onclick={() => { scrollMode = 'wrapped'; showDisplayMenu = false; }}>
								<svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none"><rect x="3" y="3" width="7" height="7" rx="1"></rect><rect x="14" y="3" width="7" height="7" rx="1"></rect><rect x="3" y="14" width="7" height="7" rx="1"></rect><rect x="14" y="14" width="7" height="7" rx="1"></rect></svg>
								Wrapped Scrolling
							</button>
						</div>
						<div class="menu-divider"></div>
						<div class="menu-group">
							<button class:active={spreadMode === 'none'} onclick={() => { spreadMode = 'none'; scrollMode = scrollMode === 'page' ? 'vertical' : scrollMode; showDisplayMenu = false; }}>
								<svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none"><rect x="7" y="3" width="10" height="18" rx="2"></rect></svg>
								No Spreads
							</button>
							<button class:active={spreadMode === 'odd'} onclick={() => { spreadMode = 'odd'; scrollMode = 'vertical'; showDisplayMenu = false; }}>
								<svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none"><rect x="3" y="3" width="8" height="18" rx="1"></rect><rect x="13" y="3" width="8" height="18" rx="1"></rect></svg>
								Odd Spreads
							</button>
							<button class:active={spreadMode === 'even'} onclick={() => { spreadMode = 'even'; scrollMode = 'vertical'; showDisplayMenu = false; }}>
								<svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none"><rect x="3" y="3" width="8" height="18" rx="1"></rect><rect x="13" y="3" width="8" height="18" rx="1"></rect></svg>
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
			<svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none"><path d="M15 3h6v6M9 21H3v-6M21 3l-7 7M3 21l7-7"></path></svg>
		</button>

		<button onclick={zoomOut}>-</button>
		<span style="white-space: nowrap; width: 40px; text-align: center;">{Math.round(scale * 100)}%</span>
		<button onclick={zoomIn}>+</button>

		{#if showAttachButton && onAttachNote && !pdfCompact}
			<div class="divider" style="width: 1px; height: 16px; background: var(--border-default); margin: 0 4px;"></div>
			<button class="attach-note-btn" onclick={onAttachNote} title="Attach Note">
				<svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none"><path d="M12 20h9"></path><path d="M16.5 3.5a2.121 2.121 0 0 1 3 3L7 19l-4 1 1-4L16.5 3.5z"></path></svg>
				<span style="margin-left: 4px;">Attach Note</span>
			</button>
		{/if}

		{#if pdfCompact}
			<div class="more-menu-container">
				<button class="more-btn" class:has-active-tool={toolMode !== 'select'} onclick={(e) => { e.stopPropagation(); showMoreMenu = !showMoreMenu; }} title="More Tools">
					<svg viewBox="0 0 24 24" width="16" height="16" fill="currentColor" stroke="none" xmlns="http://www.w3.org/2000/svg"><circle cx="5" cy="12" r="1.5"></circle><circle cx="12" cy="12" r="1.5"></circle><circle cx="19" cy="12" r="1.5"></circle></svg>
				</button>
				{#if showMoreMenu}
					<div class="dropdown more-dropdown">
						{#if showAttachButton && onAttachNote}
							<div class="menu-group">
								<button class="attach-btn-inline" onclick={() => { onAttachNote!(); showMoreMenu = false; }}>
									<svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none"><path d="M12 20h9"></path><path d="M16.5 3.5a2.121 2.121 0 0 1 3 3L7 19l-4 1 1-4L16.5 3.5z"></path></svg>
									Attach Note
								</button>
							</div>
							<div class="menu-divider"></div>
						{/if}
						<div class="menu-group">
							<button class:active={toolMode === 'select'} onclick={() => { toolMode = 'select'; showMoreMenu = false; }}>
								<svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none"><path d="M3 3l7.07 16.97 2.51-7.39 7.39-2.51L3 3z"></path><path d="M13 13l6 6"></path></svg>
								Select
							</button>
							<button class:active={toolMode === 'pen'} onclick={() => { toolMode = 'pen'; showMoreMenu = false; }}>
								<svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none"><path d="M12 19l7-7 3 3-7 7-3-3z"></path><path d="M18 13l-1.5-7.5L2 2l3.5 14.5L13 18l5-5z"></path><path d="M2 2l7.586 7.586"></path><circle cx="11" cy="11" r="2"></circle></svg>
								Draw
							</button>
							<button class:active={toolMode === 'eraser'} onclick={() => { toolMode = 'eraser'; showMoreMenu = false; }}>
								<svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none"><path d="M20 20H7L3 16C2.5 15.5 2.5 14.5 3 14L13 4C13.5 3.5 14.5 3.5 15 4L20 9C20.5 9.5 20.5 10.5 20 11L11 20H20"></path></svg>
								Eraser
							</button>
							<button class:active={toolMode === 'marquee'} onclick={() => { toolMode = 'marquee'; showMoreMenu = false; }}>
								<svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none"><rect x="3" y="3" width="18" height="18" rx="2" ry="2" stroke-dasharray="4 4"></rect></svg>
								Marquee
							</button>
						</div>
						<div class="menu-divider"></div>
						<div class="menu-group">
							<button class:active={scrollMode === 'page' && spreadMode === 'none'} onclick={() => { scrollMode = 'page'; spreadMode = 'none'; showMoreMenu = false; }}>
								<svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none"><rect x="7" y="3" width="10" height="18" rx="2"></rect><path d="M7 8h10M7 16h10"></path></svg>
								Page Scrolling
							</button>
							<button class:active={scrollMode === 'vertical'} onclick={() => { scrollMode = 'vertical'; showMoreMenu = false; }}>
								<svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none"><rect x="7" y="3" width="10" height="18" rx="2"></rect><path d="M12 3v18"></path></svg>
								Vertical Scrolling
							</button>
							<button class:active={scrollMode === 'horizontal'} onclick={() => { scrollMode = 'horizontal'; showMoreMenu = false; }}>
								<svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none"><rect x="3" y="7" width="18" height="10" rx="2"></rect><path d="M3 12h18"></path></svg>
								Horizontal Scrolling
							</button>
							<button class:active={scrollMode === 'wrapped'} onclick={() => { scrollMode = 'wrapped'; showMoreMenu = false; }}>
								<svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none"><rect x="3" y="3" width="7" height="7" rx="1"></rect><rect x="14" y="3" width="7" height="7" rx="1"></rect><rect x="3" y="14" width="7" height="7" rx="1"></rect><rect x="14" y="14" width="7" height="7" rx="1"></rect></svg>
								Wrapped Scrolling
							</button>
						</div>
					</div>
				{/if}
			</div>
		{/if}

		{#if onClosePdf}
			<div class="divider" style="width: 1px; height: 16px; background: var(--border-default); margin: 0 4px;"></div>
			<button class="close-pdf-btn" onclick={onClosePdf} title="Close PDF" aria-label="Close PDF">
				<svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round"><line x1="18" y1="6" x2="6" y2="18"></line><line x1="6" y1="6" x2="18" y2="18"></line></svg>
			</button>
		{/if}
	</div>

	<div class="pdf-viewer-scroll-area" bind:this={pdfViewerDiv}>
		{#if errorMessage}
			<div style="color: red; padding: 2rem; background: #fee2e2; border-radius: 8px; margin: 2rem;">
				<h3 style="margin-top: 0;">Error Loading PDF</h3>
				<pre style="white-space: pre-wrap; font-family: monospace;">{errorMessage}</pre>
			</div>
		{/if}

		{#if pdfDoc && defaultViewport}
			<div class="pdf-pages-container layout-{scrollMode} spread-{spreadMode}">
				{#each Array(numPages) as _, i}
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
						onPointerDown={handlePointerDown}
						onPointerMove={handlePointerMove}
						onPointerUp={handlePointerUp}
						{pdfViewerDiv}
						{defaultViewport}
					/>
				{/each}
			</div>
		{/if}

		{#if showToolbar && toolMode === 'select'}
			<div class="floating-toolbar" style={toolbarStyle}>
				<div class="swatch-group">
					<button class="swatch-btn" aria-label="Highlight Yellow" style="--swatch-color: rgba(253, 224, 71, 0.4); --swatch-border: #eab308;" onclick={() => applyHighlight('rgba(253, 224, 71, 0.4)')}><span class="swatch-icon">T</span></button>
					<button class="swatch-btn" aria-label="Highlight Green" style="--swatch-color: rgba(134, 239, 172, 0.4); --swatch-border: #22c55e;" onclick={() => applyHighlight('rgba(134, 239, 172, 0.4)')}><span class="swatch-icon">T</span></button>
					<button class="swatch-btn" aria-label="Highlight Blue" style="--swatch-color: rgba(147, 197, 253, 0.4); --swatch-border: #3b82f6;" onclick={() => applyHighlight('rgba(147, 197, 253, 0.4)')}><span class="swatch-icon">T</span></button>
					<button class="swatch-btn" aria-label="Highlight Pink" style="--swatch-color: rgba(249, 168, 212, 0.4); --swatch-border: #ec4899;" onclick={() => applyHighlight('rgba(249, 168, 212, 0.4)')}><span class="swatch-icon">T</span></button>
					<button class="swatch-btn" aria-label="Highlight Purple" style="--swatch-color: rgba(196, 181, 253, 0.4); --swatch-border: #a855f7;" onclick={() => applyHighlight('rgba(196, 181, 253, 0.4)')}><span class="swatch-icon">T</span></button>
					<button class="swatch-btn" aria-label="Highlight Orange" style="--swatch-color: rgba(253, 186, 116, 0.4); --swatch-border: #f97316;" onclick={() => applyHighlight('rgba(253, 186, 116, 0.4)')}><span class="swatch-icon">T</span></button>
				</div>
				<div class="divider"></div>
				<button class="action-btn" onclick={handleCopy} aria-label="Copy Text" title="Copy">
					<svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none"><rect x="9" y="9" width="13" height="13" rx="2" ry="2"></rect><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"></path></svg>
				</button>
				<button class="action-btn" onclick={handleQuote} aria-label="Quote to Note" title="Quote to Note">
					<svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none">
						<path d="M11 4H4a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7"></path>
						<path d="M18.5 2.5a2.121 2.121 0 0 1 3 3L12 15l-4 1 1-4 9.5-9.5z"></path>
					</svg>
				</button>
			</div>
		{/if}
	</div>
</div>

<style>
	.pdf-wrapper {
		display: flex;
		flex-direction: column;
		height: 100%;
		background: var(--bg-body);
		border-right: 1px solid var(--border-default);
		position: relative;
	}

	.pdf-controls {
		display: flex;
		align-items: center;
		gap: 0.4rem;
		padding: 0 var(--space-4);
		height: 48px;
		box-sizing: border-box;
		background: var(--bg-panel);
		border-bottom: 1px solid var(--border-default);
		font-family: var(--font-mono);
		font-size: 0.85rem;
		z-index: 100;
		overflow-x: auto;
		scrollbar-width: none; /* keep the 48px bar clean; padding reduction usually avoids the need */
	}
	.pdf-controls::-webkit-scrollbar {
		display: none;
	}

	.pdf-controls button {
		background: var(--bg-surface);
		border: 1px solid var(--border-default);
		color: var(--text-primary);
		padding: 0.25rem 0.5rem;
		border-radius: var(--radius-sm);
		cursor: pointer;
		display: flex;
		align-items: center;
		justify-content: center;
		flex-shrink: 0;
	}

	/* Close button: neutral toolbar styling (not a red standout), turns red on hover. */
	.close-pdf-btn {
		color: var(--text-secondary);
	}
	.close-pdf-btn:hover {
		color: var(--danger, #ef4444);
		border-color: var(--danger, #ef4444) !important;
	}

	.display-menu-container {
		position: relative;
	}

	.display-btn {
		padding: 0.35rem 0.5rem !important;
	}

	.dropdown {
		position: absolute;
		top: 100%;
		left: 0;
		margin-top: 0.5rem;
		background: var(--bg-panel);
		border: 1px solid var(--border-default);
		border-radius: var(--radius-md);
		box-shadow: 0 4px 12px rgba(0,0,0,0.2);
		padding: 0.5rem;
		display: flex;
		flex-direction: column;
		gap: 0.5rem;
		min-width: 180px;
	}

	.menu-group {
		display: flex;
		flex-direction: column;
		gap: 0.25rem;
	}

	.menu-group button {
		justify-content: flex-start;
		gap: 0.5rem;
		border: none;
		background: transparent;
		padding: 0.35rem 0.5rem;
		font-size: 0.85rem;
	}

	.menu-group button:hover {
		background: var(--bg-hover);
	}

	.menu-group button.active {
		background: var(--accent-100);
		color: var(--text-inverse);
	}

	.menu-divider {
		height: 1px;
		background: var(--border-default);
		margin: 0.25rem 0;
	}

	.attach-note-btn {
		background: var(--accent-100) !important;
		color: var(--text-inverse) !important;
		border: none !important;
		font-weight: 600;
	}

	.vertical-divider {
		width: 1px;
		height: 24px;
		background: var(--border-default);
		margin: 0 0.25rem;
	}


	.tools-group {
		display: flex;
		gap: 0.25rem;
	}

	.tools-group button.active {
		background: var(--accent-100);
		border-color: var(--accent-200);
		color: var(--text-inverse);
	}

	.spacer { flex: 1; }

	.pdf-viewer-scroll-area {
		flex: 1;
		overflow: auto;
		position: relative;
		padding: 2rem;
		display: flex;
		flex-direction: column;
		align-items: center;
	}

	/* Layout Modes */
	.pdf-pages-container {
		display: flex;
		gap: 1rem;
		margin: 0 auto;
	}

	/* Vertical */
	.pdf-pages-container.layout-vertical {
		flex-direction: column;
		align-items: center;
	}

	/* Horizontal */
	.pdf-pages-container.layout-horizontal {
		flex-direction: row;
		align-items: center;
	}

	/* Wrapped */
	.pdf-pages-container.layout-wrapped {
		flex-direction: row;
		flex-wrap: wrap;
		justify-content: center;
		max-width: 100%;
	}

	/* Page (Single Page view with snappy scroll is hard natively, so we just use Vertical + snap or just Vertical) */
	.pdf-pages-container.layout-page {
		flex-direction: column;
		align-items: center;
	}

	/* Spreads */
	.pdf-pages-container.spread-even,
	.pdf-pages-container.spread-odd {
		display: grid;
		grid-template-columns: 1fr 1fr;
		gap: 1rem;
		justify-content: center;
	}

	.pdf-pages-container.spread-odd :global(> .pdf-page-container:first-child) {
		grid-column: 2; /* Forces the first page to be on the right side */
	}

	/* Fix for text layer making text visible on selection */
	:global(.textLayer ::selection) {
		color: transparent !important;
	}
	:global(.textLayer ::-moz-selection) {
		color: transparent !important;
	}

	.floating-toolbar {
		position: absolute;
		background: var(--bg-panel);
		border: 1px solid var(--border-default);
		border-radius: var(--radius-md);
		box-shadow: 0 4px 12px rgba(0,0,0,0.2);
		z-index: 100;
		padding: 0.25rem;
		display: flex;
		align-items: center;
		gap: 0.25rem;
	}

	.swatch-group { display: flex; gap: 0.25rem; padding: 0 0.25rem; }
	.swatch-btn {
		width: 24px; height: 24px; border-radius: var(--radius-sm); border: 1px solid transparent;
		background: transparent; display: flex; align-items: center; justify-content: center;
		cursor: pointer; position: relative; transition: transform 0.1s ease;
	}
	.swatch-btn:hover { transform: scale(1.1); }
	.swatch-icon {
		font-weight: 700; font-family: serif; font-size: 14px; color: var(--text-primary);
		line-height: 1; border-bottom: 3px solid var(--swatch-border); padding-bottom: 1px;
	}
	.divider { width: 1px; height: 20px; background: var(--border-default); margin: 0 0.25rem; }
	.action-btn {
		display: flex; align-items: center; justify-content: center; width: 28px; height: 28px;
		background: transparent; color: var(--text-secondary); border: none; border-radius: var(--radius-sm);
		cursor: pointer; transition: background 0.15s ease, color 0.15s ease;
	}
	.action-btn:hover { background: var(--bg-hover); color: var(--text-primary); }

	/* More-tools dropdown */
	.more-menu-container {
		position: relative;
	}

	.more-btn {
		padding: 0.35rem 0.5rem !important;
		color: var(--text-secondary);
	}

	.more-btn.has-active-tool {
		color: var(--accent-100);
		border-color: var(--accent-200) !important;
	}

	.more-dropdown {
		right: 0;
		left: auto;
		min-width: 160px;
	}

	.attach-btn-inline {
		display: flex;
		align-items: center;
		gap: 0.5rem;
		justify-content: flex-start !important;
		background: color-mix(in srgb, var(--accent-100) 15%, transparent) !important;
		color: var(--accent-100) !important;
		border: 1px solid color-mix(in srgb, var(--accent-200) 40%, transparent) !important;
		font-weight: 600;
		width: 100%;
		padding: 0.35rem 0.5rem;
		border-radius: var(--radius-sm);
		cursor: pointer;
		font-size: 0.85rem;
	}

	.attach-btn-inline:hover {
		background: color-mix(in srgb, var(--accent-100) 25%, transparent) !important;
	}
</style>
