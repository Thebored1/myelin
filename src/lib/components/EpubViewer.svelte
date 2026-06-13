<script lang="ts">
	import { onMount, onDestroy } from 'svelte';
	import ePub from 'epubjs';

	interface Props {
		epubBytes: Uint8Array;
	}
	let { epubBytes }: Props = $props();

	let container: HTMLDivElement | undefined = $state();
	let book: any = null;
	let rendition: any = null;

	onMount(() => {
		if (container && epubBytes) {
			book = ePub(epubBytes.buffer as ArrayBuffer);
			rendition = book.renderTo(container, {
				width: '100%',
				height: '100%',
				spread: 'none'
			});
			rendition.display();
		}
	});

	onDestroy(() => {
		if (book) {
			book.destroy();
		}
	});
</script>

<div style="width: 100%; height: 100%; display: flex; flex-direction: column;">
	<div class="controls" style="padding: 8px; display: flex; gap: 8px; justify-content: center; background: var(--bg-panel); border-bottom: 1px solid var(--border-default);">
		<button class="secondary" onclick={() => rendition && rendition.prev()}>Prev</button>
		<button class="secondary" onclick={() => rendition && rendition.next()}>Next</button>
	</div>
	<div bind:this={container} style="flex: 1; overflow: hidden; background: white;"></div>
</div>
