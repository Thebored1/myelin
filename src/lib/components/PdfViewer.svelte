<script lang="ts">
	import PdfViewerRuntime from '$lib/pdf/components/PdfViewerRuntime.svelte';
	import type { PdfViewerProps } from '$lib/pdf/types';

	let {
		pdfBytes,
		annotations,
		onQuote,
		onAnnotationsChange,
		onImageExtract,
		onTextExtracted,
		onActiveSection,
		onSectionsReady,
		onAttachNote,
		onClosePdf,
		showAttachButton
	}: PdfViewerProps = $props();
</script>

<PdfViewerRuntime
	{pdfBytes}
	{annotations}
	{onQuote}
	{onAnnotationsChange}
	{onImageExtract}
	{onTextExtracted}
	{onActiveSection}
	{onSectionsReady}
	{onAttachNote}
	{onClosePdf}
	{showAttachButton}
/>
