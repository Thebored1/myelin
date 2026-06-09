<script lang="ts">
	import { onMount, onDestroy } from 'svelte';
	import * as pdfjsLib from 'pdfjs-dist';
	import 'pdfjs-dist/build/pdf.worker.mjs';
	import 'pdfjs-dist/web/pdf_viewer.css';
	import type { PdfAnnotation } from '$lib/types';
	

	// The worker is loaded via Vite config or directly from dist
	pdfjsLib.GlobalWorkerOptions.workerSrc = new URL(
		'pdfjs-dist/build/pdf.worker.mjs',
		import.meta.url
	).toString();

	let { 
		pdfBytes, 
		annotations = [], 
		onQuote,
		onAnnotationsChange,
		onImageExtract,
		onAttachNote,
		showAttachButton
	}: { 
		pdfBytes: Uint8Array, 
		annotations?: PdfAnnotation[],
		onQuote: (text: string, page: number) => void,
		onAnnotationsChange?: (anns: PdfAnnotation[]) => void,
		onImageExtract?: (base64: string) => void,
		onAttachNote?: () => void,
		showAttachButton?: boolean
	} = $props();

	let canvas: HTMLCanvasElement | undefined = $state();
	let textLayerDiv: HTMLDivElement | undefined = $state();
	let containerDiv: HTMLDivElement | undefined = $state();
	let pdfViewerDiv: HTMLDivElement | undefined = $state();
	
	let pdfDoc: any = null;
	let pageNum = $state(1);
	let pageRendering = false;
	let pageNumPending: number | null = null;
	let numPages = $state(0);
	let scale = $state(1.2);
	let scaleInitialized = false;
	
	let pageViewport: any = $state(null);

	let selectionText = '';
	let showToolbar = $state(false);
	let toolbarStyle = $state('');

	// Tools State
	type ToolMode = 'select' | 'pen' | 'eraser' | 'marquee';
	let toolMode = $state<ToolMode>('select');
	let isDrawing = $state(false);
	let currentPath = $state<[number, number][]>([]);
	let currentRect = $state<[number, number, number, number] | null>(null);
	let selectionRects = $state<[number, number, number, number][]>([]);
	
	let pageAnnotations = $derived(annotations.filter(a => a.page === pageNum));

	async function loadPdf() {
		try {
			const loadingTask = pdfjsLib.getDocument({ data: pdfBytes });
			pdfDoc = await loadingTask.promise;
			numPages = pdfDoc.numPages;
			scaleInitialized = false;
			renderPage(pageNum);
		} catch (error) {
			console.error('Error loading PDF:', error);
		}
	}

	async function renderPage(num: number) {
		pageRendering = true;
		try {
			const page = await pdfDoc.getPage(num);
			
			if (!scaleInitialized && containerDiv) {
				const unscaledViewport = page.getViewport({ scale: 1.0 });
				const targetWidth = containerDiv.clientWidth - 64; 
				if (targetWidth > 0) {
					scale = targetWidth / unscaledViewport.width;
				}
				scaleInitialized = true;
			}
			
			const viewport = page.getViewport({ scale });
			pageViewport = viewport;

			if (canvas) {
				canvas.height = viewport.height;
				canvas.width = viewport.width;

				const renderContext = {
					canvasContext: canvas.getContext('2d') as CanvasRenderingContext2D,
					viewport: viewport
				};

				await page.render(renderContext).promise;
			}

			if (textLayerDiv) {
				textLayerDiv.innerHTML = '';
				textLayerDiv.style.setProperty('--scale-factor', scale.toString());
				textLayerDiv.style.width = `${viewport.width}px`;
				textLayerDiv.style.height = `${viewport.height}px`;

				const textContent = await page.getTextContent();
				
				const textLayer = new pdfjsLib.TextLayer({
					textContentSource: textContent,
					container: textLayerDiv,
					viewport: viewport
				});
				await textLayer.render();
			}

		} catch (error) {
			console.error('Error rendering page:', error);
		}

		pageRendering = false;
		if (pageNumPending !== null) {
			renderPage(pageNumPending);
			pageNumPending = null;
		}
	}

	function queueRenderPage(num: number) {
		if (pageRendering) {
			pageNumPending = num;
		} else {
			renderPage(num);
		}
	}

	function prevPage() {
		if (pageNum <= 1) return;
		pageNum--;
		queueRenderPage(pageNum);
	}

	function nextPage() {
		if (pageNum >= numPages) return;
		pageNum++;
		queueRenderPage(pageNum);
	}

	function zoomIn() {
		scale += 0.2;
		queueRenderPage(pageNum);
	}

	function zoomOut() {
		if (scale <= 0.6) return;
		scale -= 0.2;
		queueRenderPage(pageNum);
	}

	async function fitToScreen() {
		if (!pdfDoc || !containerDiv) return;
		const page = await pdfDoc.getPage(pageNum);
		const unscaledViewport = page.getViewport({ scale: 1.0 });
		
		const targetWidth = containerDiv.clientWidth - 64; 
		
		if (targetWidth > 0) {
			scale = targetWidth / unscaledViewport.width;
			queueRenderPage(pageNum);
		}
	}

	let selectionTimeout: ReturnType<typeof setTimeout> | undefined;

	function handleSelection() {
		if (toolMode !== 'select') return;
		const selection = window.getSelection();
		console.log("handleSelection called, isCollapsed:", selection?.isCollapsed, "anchorNode:", selection?.anchorNode);
		
		if (!selection || selection.isCollapsed) {
			if (selectionTimeout) clearTimeout(selectionTimeout);
			showToolbar = false;
			return;
		}

		console.log("textLayerDiv exists:", !!textLayerDiv, "contains anchor:", textLayerDiv?.contains(selection.anchorNode));
		
		if (textLayerDiv && textLayerDiv.contains(selection.anchorNode)) {
			if (selectionTimeout) clearTimeout(selectionTimeout);
			
			selectionTimeout = setTimeout(() => {
				const currentSelection = window.getSelection();
				if (!currentSelection || currentSelection.isCollapsed) return;
				
				selectionText = currentSelection.toString().trim();
				if (selectionText) {
					const range = currentSelection.getRangeAt(0);
					const rects = Array.from(range.getClientRects());

					// Get the SVG annotation layer element — this IS the coordinate
					// system we render highlights into, so using its own bounding rect
					// as origin guarantees pixel-perfect alignment.
					const svgLayer = canvas?.parentElement?.querySelector('.annotation-layer');
					
					if (pdfViewerDiv && canvas && svgLayer) {
						const viewerRect = pdfViewerDiv.getBoundingClientRect();
						const svgRect = svgLayer.getBoundingClientRect();

						const rect = range.getBoundingClientRect();
						const top = rect.top - viewerRect.top + pdfViewerDiv.scrollTop - 40;
						const left = rect.left - viewerRect.left + pdfViewerDiv.scrollLeft + (rect.width / 2) - 100;

						toolbarStyle = `top: ${top}px; left: ${left}px;`;
						
						selectionRects = rects.map(r => {
							// Compute the TRUE scale from SVG's actual rendered size vs viewBox.
							// Browser sub-pixel rounding can make this differ from `scale`.
							const svgEl = svgLayer as unknown as SVGSVGElement;
							const vb = svgEl.viewBox.baseVal;
							const realScaleX = svgRect.width / vb.width;
							const realScaleY = svgRect.height / vb.height;
							
							const unscaledX = (r.left - svgRect.left) / realScaleX;
							const unscaledY = (r.top - svgRect.top) / realScaleY;
							const unscaledW = r.width / realScaleX;
							const unscaledH = r.height / realScaleY;
							return [unscaledX, unscaledY, unscaledW, unscaledH];
						});
					}
					showToolbar = true;
				} else {
					showToolbar = false;
				}
			}, 350);
		} else {
			showToolbar = false;
		}
	}

	function handleQuote() {
		if (selectionText && onQuote) {
			onQuote(selectionText, pageNum);
			window.getSelection()?.removeAllRanges();
			showToolbar = false;
		}
	}
	
	function applyHighlight(color: string) {
		if (selectionText && selectionRects.length > 0) {
			const newAnn: PdfAnnotation = {
				id: Math.random().toString(36).substring(7),
				page: pageNum,
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

	// Pointer event handling for annotations
	function getUnscaledCoords(e: PointerEvent, element: HTMLElement): [number, number] {
		const rect = element.getBoundingClientRect();
		const x = (e.clientX - rect.left) / scale;
		const y = (e.clientY - rect.top) / scale;
		return [x, y];
	}

	function handlePointerDown(e: PointerEvent) {
		if (toolMode === 'select') return;
		const target = e.currentTarget as HTMLElement;
		target.setPointerCapture(e.pointerId);
		isDrawing = true;

		const coords = getUnscaledCoords(e, target);
		
		if (toolMode === 'pen') {
			currentPath = [coords];
		} else if (toolMode === 'marquee') {
			currentRect = [coords[0], coords[1], 0, 0];
		} else if (toolMode === 'eraser') {
			eraseAt(coords);
		}
	}

	function handlePointerMove(e: PointerEvent) {
		if (!isDrawing || toolMode === 'select') return;
		const target = e.currentTarget as HTMLElement;
		const coords = getUnscaledCoords(e, target);

		if (toolMode === 'pen') {
			currentPath = [...currentPath, coords];
		} else if (toolMode === 'marquee' && currentRect) {
			const startX = currentRect[0];
			const startY = currentRect[1];
			currentRect = [startX, startY, coords[0] - startX, coords[1] - startY];
		} else if (toolMode === 'eraser') {
			eraseAt(coords);
		}
	}

	function handlePointerUp(e: PointerEvent) {
		if (!isDrawing || toolMode === 'select') return;
		const target = e.currentTarget as HTMLElement;
		target.releasePointerCapture(e.pointerId);
		isDrawing = false;

		if (toolMode === 'pen' && currentPath.length > 1) {
			const newAnn: PdfAnnotation = {
				id: crypto.randomUUID(),
				page: pageNum,
				type: 'draw',
				points: currentPath,
				color: '#ef4444',
				strokeWidth: 2
			};
			if (onAnnotationsChange) onAnnotationsChange([...annotations, newAnn]);
		} else if (toolMode === 'marquee' && currentRect && onImageExtract && canvas) {
			// Extract image from canvas
			const [x, y, w, h] = currentRect;
			const realX = Math.min(x, x + w) * scale;
			const realY = Math.min(y, y + h) * scale;
			const realW = Math.abs(w) * scale;
			const realH = Math.abs(h) * scale;

			if (realW > 10 && realH > 10) {
				const cropCanvas = document.createElement('canvas');
				cropCanvas.width = realW;
				cropCanvas.height = realH;
				const ctx = cropCanvas.getContext('2d');
				if (ctx) {
					ctx.drawImage(canvas, realX, realY, realW, realH, 0, 0, realW, realH);
					const base64 = cropCanvas.toDataURL('image/png');
					onImageExtract(base64);
				}
			}
		}

		currentPath = [];
		currentRect = null;
	}

	function eraseAt([x, y]: [number, number]) {
		const ERASER_RADIUS = 15 / scale;
		let modified = false;
		const filtered = annotations.filter(ann => {
			if (ann.page !== pageNum || !ann.points) return true;
			// Check if any point is within radius
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

	function generateSvgPath(points: [number, number][]): string {
		if (!points || points.length === 0) return '';
		const d = points.map((p, i) => (i === 0 ? `M ${p[0]} ${p[1]}` : `L ${p[0]} ${p[1]}`)).join(' ');
		return d;
	}

	$effect(() => {
		if (pdfBytes) {
			loadPdf();
		}
	});

	onMount(() => {
		document.addEventListener('selectionchange', handleSelection);
	});

	onDestroy(() => {
		document.removeEventListener('selectionchange', handleSelection);
	});
</script>

<div class="pdf-wrapper" bind:this={containerDiv}>
	<div class="pdf-controls">
		{#if showAttachButton && onAttachNote}
			<button class="attach-note-btn" onclick={onAttachNote} title="Attach Note">
				<svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none"><path d="M12 20h9"></path><path d="M16.5 3.5a2.121 2.121 0 0 1 3 3L7 19l-4 1 1-4L16.5 3.5z"></path></svg>
				<span style="margin-left: 4px;">Attach Note</span>
			</button>
			<div class="vertical-divider"></div>
		{/if}
		<button onclick={prevPage} disabled={pageNum <= 1}>Previous</button>
		<span style="white-space: nowrap;">Page {pageNum} of {numPages}</span>
		<button onclick={nextPage} disabled={pageNum >= numPages}>Next</button>
		<div class="spacer"></div>
		
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

		<div class="spacer"></div>
		<button onclick={fitToScreen} title="Fit to Screen">
			<svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none"><path d="M15 3h6v6M9 21H3v-6M21 3l-7 7M3 21l7-7"></path></svg>
		</button>
		<button onclick={zoomOut}>-</button>
		<span style="white-space: nowrap;">{Math.round(scale * 100)}%</span>
		<button onclick={zoomIn}>+</button>
	</div>

	<div class="pdf-viewer pdfViewer" bind:this={pdfViewerDiv}>
		<div class="pdf-page-container page" style="--scale-factor: {scale}; width: {pageViewport?.width || 800}px; height: {pageViewport?.height || 1000}px;">
			<canvas bind:this={canvas}></canvas>
			<div class="textLayer" bind:this={textLayerDiv}></div>
			
			<!-- Annotation Layer -->
			<svg 
				class="annotation-layer" 
				class:active={toolMode !== 'select'}
				onpointerdown={handlePointerDown}
				onpointermove={handlePointerMove}
				onpointerup={handlePointerUp}
				onpointerleave={handlePointerUp}
				viewBox="0 0 {(pageViewport?.width || 800) / scale} {(pageViewport?.height || 1000) / scale}"
				preserveAspectRatio="none"
			>
				{#each pageAnnotations as ann (ann.id)}
					{#if ann.type === 'draw' || ann.type === 'highlight'}
						{#if ann.points}
							<path 
								d={generateSvgPath(ann.points)} 
								stroke={ann.color} 
								stroke-width={ann.strokeWidth} 
								fill="none" 
								stroke-linecap="round" 
								stroke-linejoin="round" 
							/>
						{/if}
					{:else if ann.type === 'text_highlight'}
						{#if ann.rects}
							{#each ann.rects as rect}
								<rect 
									x={rect[0]} 
									y={rect[1]} 
									width={rect[2]} 
									height={rect[3]} 
									fill={ann.color} 
									style="mix-blend-mode: multiply;"
								/>
							{/each}
						{/if}
					{/if}
				{/each}

				<!-- Active drawing path -->
				{#if toolMode === 'pen' && currentPath.length > 0}
					<path 
						d={generateSvgPath(currentPath)} 
						stroke="#ef4444" 
						stroke-width="2" 
						fill="none" 
						stroke-linecap="round" 
						stroke-linejoin="round" 
					/>
				{/if}

				<!-- Active marquee rect -->
				{#if toolMode === 'marquee' && currentRect}
					<rect 
						x={Math.min(currentRect[0], currentRect[0] + currentRect[2])} 
						y={Math.min(currentRect[1], currentRect[1] + currentRect[3])} 
						width={Math.abs(currentRect[2])} 
						height={Math.abs(currentRect[3])} 
						fill="rgba(59, 130, 246, 0.2)" 
						stroke="#3b82f6" 
						stroke-width="1"
						stroke-dasharray="4"
					/>
				{/if}
			</svg>
		</div>
		
		{#if showToolbar && toolMode === 'select'}
			<div class="floating-toolbar" style={toolbarStyle}>
				<!-- Highlighters -->
				<div class="swatch-group">
					<button class="swatch-btn" aria-label="Highlight Yellow" style="--swatch-color: rgba(253, 224, 71, 0.4); --swatch-border: #eab308;" onclick={() => applyHighlight('rgba(253, 224, 71, 0.4)')}>
						<span class="swatch-icon">T</span>
					</button>
					<button class="swatch-btn" aria-label="Highlight Green" style="--swatch-color: rgba(134, 239, 172, 0.4); --swatch-border: #22c55e;" onclick={() => applyHighlight('rgba(134, 239, 172, 0.4)')}>
						<span class="swatch-icon">T</span>
					</button>
					<button class="swatch-btn" aria-label="Highlight Blue" style="--swatch-color: rgba(147, 197, 253, 0.4); --swatch-border: #3b82f6;" onclick={() => applyHighlight('rgba(147, 197, 253, 0.4)')}>
						<span class="swatch-icon">T</span>
					</button>
					<button class="swatch-btn" aria-label="Highlight Pink" style="--swatch-color: rgba(249, 168, 212, 0.4); --swatch-border: #ec4899;" onclick={() => applyHighlight('rgba(249, 168, 212, 0.4)')}>
						<span class="swatch-icon">T</span>
					</button>
					<button class="swatch-btn" aria-label="Highlight Purple" style="--swatch-color: rgba(196, 181, 253, 0.4); --swatch-border: #a855f7;" onclick={() => applyHighlight('rgba(196, 181, 253, 0.4)')}>
						<span class="swatch-icon">T</span>
					</button>
					<button class="swatch-btn" aria-label="Highlight Orange" style="--swatch-color: rgba(253, 186, 116, 0.4); --swatch-border: #f97316;" onclick={() => applyHighlight('rgba(253, 186, 116, 0.4)')}>
						<span class="swatch-icon">T</span>
					</button>
				</div>
				<div class="divider"></div>
				<button class="action-btn" onclick={handleCopy} aria-label="Copy Text" title="Copy">
					<svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round"><rect x="9" y="9" width="13" height="13" rx="2" ry="2"></rect><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"></path></svg>
				</button>
				<button class="action-btn" onclick={handleQuote} aria-label="Quote to Note" title="Quote to Note">
					<svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
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
		gap: 0.5rem;
		padding: 0 0.5rem;
		height: 48px;
		box-sizing: border-box;
		overflow-x: auto;
		overflow-y: hidden;
		white-space: nowrap;
		background: var(--bg-panel);
		border-bottom: 1px solid var(--border-default);
		font-family: var(--font-mono);
		font-size: 0.85rem;
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

	.pdf-controls button:disabled {
		opacity: 0.5;
		cursor: not-allowed;
	}

	.tools-group {
		display: flex;
		gap: 0.25rem;
		padding: 0 0.5rem;
		border-left: 1px solid var(--border-default);
		border-right: 1px solid var(--border-default);
	}

	.tools-group button.active {
		background: var(--accent-100);
		border-color: var(--accent-200);
		color: var(--text-inverse);
	}

	.spacer {
		flex: 1;
	}

	.pdf-viewer {
		flex: 1;
		overflow: auto;
		position: relative;
		display: flex;
		justify-content: center;
		padding: 2rem;
	}

	.pdf-page-container {
		position: relative;
		box-shadow: 0 4px 12px rgba(0,0,0,0.1);
		background: white;
	}

	.pdf-page-container canvas {
		display: block;
	}

	.annotation-layer {
		position: absolute;
		top: 0;
		left: 0;
		width: 100%;
		height: 100%;
		pointer-events: none;
		z-index: 10;
		touch-action: none;
	}

	.annotation-layer.active {
		pointer-events: auto;
	}

	:global(.textLayer) {
		position: absolute;
		text-align: initial;
		left: 0;
		top: 0;
		right: 0;
		bottom: 0;
		overflow: hidden;
		opacity: 0.99; /* Set >0 to allow selection to show */
		line-height: 1.0;
		text-size-adjust: none;
		forced-color-adjust: none;
		transform-origin: 0 0;
		z-index: 2;
	}

	:global(.textLayer span),
	:global(.textLayer br) {
		color: transparent;
		position: absolute;
		white-space: pre;
		cursor: text;
		transform-origin: 0% 0%;
		margin: 0;
		padding: 0;
	}

	:global(.textLayer ::selection) {
		color: transparent !important;
		background: rgba(238, 96, 24, 0.3) !important;
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

	.swatch-group {
		display: flex;
		gap: 0.25rem;
		padding: 0 0.25rem;
	}

	.swatch-btn {
		width: 24px;
		height: 24px;
		border-radius: var(--radius-sm);
		border: 1px solid transparent;
		background: transparent;
		display: flex;
		align-items: center;
		justify-content: center;
		cursor: pointer;
		position: relative;
		transition: transform 0.1s ease;
	}

	.swatch-btn:hover {
		transform: scale(1.1);
	}

	.swatch-icon {
		font-weight: 700;
		font-family: serif;
		font-size: 14px;
		color: var(--text-primary);
		line-height: 1;
		border-bottom: 3px solid var(--swatch-border);
		padding-bottom: 1px;
	}

	.divider {
		width: 1px;
		height: 20px;
		background: var(--border-default);
		margin: 0 0.25rem;
	}

	.action-btn {
		display: flex;
		align-items: center;
		justify-content: center;
		width: 28px;
		height: 28px;
		background: transparent;
		color: var(--text-secondary);
		border: none;
		border-radius: var(--radius-sm);
		cursor: pointer;
		transition: background 0.15s ease, color 0.15s ease;
	}
	
	.action-btn:hover {
		background: var(--bg-hover);
		color: var(--text-primary);
	}
</style>
