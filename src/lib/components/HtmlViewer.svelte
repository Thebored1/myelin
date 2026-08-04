<script lang="ts">
	interface Section {
		key: string;
		label: string;
		content: string;
	}

	interface Props {
		htmlBytes: Uint8Array;
		onActiveSection?: (section: Section) => void;
		onSectionsReady?: (sections: Section[]) => void;
	}
	let { htmlBytes, onActiveSection, onSectionsReady }: Props = $props();

	let htmlContent = $derived(new TextDecoder('utf-8').decode(htmlBytes));

	let iframeEl: HTMLIFrameElement | undefined = $state();
	let reportTimer: ReturnType<typeof setTimeout> | null = null;

	function reportViewport() {
		const doc = iframeEl?.contentDocument;
		if (!doc?.body || !onActiveSection) return;
		const scrollRoot = doc.scrollingElement ?? doc.documentElement;
		const viewport = Math.max(scrollRoot.clientHeight, 1);
		const bucket = Math.floor(scrollRoot.scrollTop / viewport);
		const text = doc.body.innerText?.trim() ?? '';
		if (text) {
			onActiveSection({
				key: `html:viewport:${bucket}`,
				label: `HTML section ${bucket + 1}`,
				content: text.slice(bucket * 12_000, (bucket + 1) * 12_000)
			});
		}
	}

	// Eager section cache: every fixed-size bucket of the document becomes a
	// cacheable section, so scrolling anywhere (and asking about anything) hits
	// a pre-computed KV snapshot instead of an inline evaluation.
	function reportAllBuckets() {
		const doc = iframeEl?.contentDocument;
		if (!doc?.body || !onSectionsReady) return;
		const text = doc.body.innerText?.trim() ?? '';
		if (!text) return;
		const CHUNK = 12_000;
		const sections: Section[] = [];
		for (let i = 0; i * CHUNK < text.length; i += 1) {
			sections.push({
				key: `html:viewport:${i}`,
				label: `HTML section ${i + 1}`,
				content: text.slice(i * CHUNK, (i + 1) * CHUNK)
			});
		}
		if (sections.length) onSectionsReady(sections);
	}

	$effect(() => {
		if (iframeEl && htmlContent) {
			iframeEl.srcdoc = htmlContent;
			iframeEl.onload = () => {
				const root = iframeEl?.contentDocument?.scrollingElement;
				root?.addEventListener('scroll', () => {
					if (reportTimer) clearTimeout(reportTimer);
					reportTimer = setTimeout(reportViewport, 100);
				}, { passive: true });
				setTimeout(reportViewport, 0);
				reportAllBuckets();
			};
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
