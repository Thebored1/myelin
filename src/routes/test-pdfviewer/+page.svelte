<script lang="ts">
    import { onMount } from 'svelte';
    import * as pdfjsLib from 'pdfjs-dist';
    import { PDFViewer, EventBus } from 'pdfjs-dist/web/pdf_viewer.mjs';
    import pdfWorkerUrl from 'pdfjs-dist/build/pdf.worker.mjs?url';
    import 'pdfjs-dist/web/pdf_viewer.css';

    pdfjsLib.GlobalWorkerOptions.workerSrc = pdfWorkerUrl;

    onMount(() => {
        console.log(PDFViewer);
    });
</script>

<div>Test</div>
