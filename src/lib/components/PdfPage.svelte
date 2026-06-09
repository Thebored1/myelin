<script lang="ts">
	import { onMount, onDestroy, untrack } from 'svelte';
	import * as pdfjsLib from 'pdfjs-dist';
	import type { PdfAnnotation } from '$lib/types';

	let {
		pdfDoc,
		pageNum,
		scale,
		annotations = [],
		toolMode,
		isDrawing,
		currentPath,
		currentRect,
		onAnnotationsChange,
		onImageExtract,
		onPointerDown,
		onPointerMove,
		onPointerUp,
		pdfViewerDiv,
		defaultViewport
	} = $props();

	let containerDiv: HTMLDivElement | undefined = $state();
	let canvas: HTMLCanvasElement | undefined = $state();
	let textLayerDiv: HTMLDivElement | undefined = $state();
	
	let pageAnnotations = $derived(annotations.filter(a => a.page === pageNum));
	
	let isVisible = $state(false);
	let hasRendered = $state(false);
	let pageViewport: any = $state.raw();
	let renderTask: any = null;

	$effect(() => {
		if (!pageViewport && defaultViewport) {
			pageViewport = defaultViewport;
		}
	});

	let observer: IntersectionObserver;

	function checkRender() {
		if (isVisible && !hasRendered && pdfDoc) {
			void renderPage();
		}
	}

	onMount(() => {
		observer = new IntersectionObserver((entries) => {
			entries.forEach(entry => {
				isVisible = entry.isIntersecting;
				if (isVisible) {
					checkRender();
				}
			});
		}, {
			rootMargin: '100% 0px 100% 0px' // Pre-render 1 full screen above and below
		});

		if (containerDiv) {
			observer.observe(containerDiv);
		}
		
		// Fallback trigger for initial load
		if (isVisible) {
			setTimeout(checkRender, 50);
		}
	});

	onDestroy(() => {
		if (observer) observer.disconnect();
		if (renderTask) renderTask.cancel();
	});

	async function renderPage() {
		if (!pdfDoc || !canvas || !textLayerDiv) {
			return;
		}
		try {
			if (renderTask) {
				renderTask.cancel();
				renderTask = null;
			}
			const page = await pdfDoc.getPage(pageNum);
			const viewport = page.getViewport({ scale });
			pageViewport = viewport;

			canvas.height = viewport.height;
			canvas.width = viewport.width;

			const renderContext = {
				canvasContext: canvas.getContext('2d') as CanvasRenderingContext2D,
				viewport: viewport
			};

			renderTask = page.render(renderContext);
			await renderTask.promise;

			textLayerDiv.innerHTML = '';
			textLayerDiv.style.setProperty('--scale-factor', scale.toString());
			textLayerDiv.style.setProperty('--total-scale-factor', scale.toString());
			textLayerDiv.style.width = `${viewport.width}px`;
			textLayerDiv.style.height = `${viewport.height}px`;

			const textContent = await page.getTextContent();
			
			const textLayer = new pdfjsLib.TextLayer({
				textContentSource: textContent,
				container: textLayerDiv,
				viewport: viewport
			});
			await textLayer.render();
			hasRendered = true;
			renderTask = null;
		} catch (error: any) {
			if (error?.name !== 'RenderingCancelledException') {
				console.error(`Error rendering page ${pageNum}:`, error);
			}
			renderTask = null;
		}
	}

	$effect(() => {
		scale;
		untrack(() => {
			hasRendered = false;
			if (isVisible && pdfDoc) {
				void renderPage();
			} else if (pageViewport && pageViewport.scale) {
				pageViewport = {
					width: (pageViewport.width / pageViewport.scale) * scale,
					height: (pageViewport.height / pageViewport.scale) * scale,
					scale
				};
			}
		});
	});

	function generateSvgPath(points: [number, number][]): string {
		if (!points || points.length === 0) return '';
		return points.map((p, i) => (i === 0 ? `M ${p[0]} ${p[1]}` : `L ${p[0]} ${p[1]}`)).join(' ');
	}
	
	function handleDown(e: PointerEvent) { onPointerDown(e, pageNum, scale); }
	function handleMove(e: PointerEvent) { onPointerMove(e, scale); }
	function handleUp(e: PointerEvent) { onPointerUp(e, pageNum, scale, canvas); }

</script>

<div 
	class="pdf-page-container page" 
	bind:this={containerDiv}
	style="--scale-factor: {scale}; --total-scale-factor: {scale}; width: {pageViewport?.width || 800}px; height: {pageViewport?.height || 1000}px;"
	data-page-number={pageNum}
>
	<canvas bind:this={canvas} class:hidden={!hasRendered}></canvas>
	<div class="textLayer" bind:this={textLayerDiv}></div>
	
	<!-- Annotation Layer -->
	<svg 
		class="annotation-layer" 
		class:active={toolMode !== 'select'}
		role="presentation"
		onpointerdown={handleDown}
		onpointermove={handleMove}
		onpointerup={handleUp}
		onpointerleave={handleUp}
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
		{#if toolMode === 'pen' && currentPath.length > 0 && isDrawing}
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
		{#if toolMode === 'marquee' && currentRect && isDrawing}
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

<style>
	.pdf-page-container {
		position: relative;
		box-shadow: 0 4px 12px rgba(0,0,0,0.1);
		background: white;
		flex-shrink: 0;
	}

	.pdf-page-container canvas {
		display: block;
	}

	.hidden {
		opacity: 0;
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

	/* We omit textLayer styles here assuming they remain global in PdfViewer.svelte */
</style>
