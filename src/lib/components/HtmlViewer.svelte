<script lang="ts">
	interface Props {
		htmlBytes: Uint8Array;
	}
	let { htmlBytes }: Props = $props();

	let htmlContent = $derived(
		new TextDecoder('utf-8').decode(htmlBytes)
	);
	
	let iframeEl: HTMLIFrameElement | undefined = $state();
	
	$effect(() => {
		if (iframeEl && htmlContent) {
			iframeEl.srcdoc = htmlContent;
		}
	});
</script>

<div style="width: 100%; height: 100%; background: white;">
	<iframe
		bind:this={iframeEl}
		sandbox="allow-same-origin allow-scripts"
		style="width: 100%; height: 100%; border: none;"
		title="HTML Viewer"
	></iframe>
</div>
