<script lang="ts">
	import { onMount, onDestroy } from 'svelte';
	import ePub from 'epubjs';

	interface Section {
		key: string;
		label: string;
		content: string;
	}

	interface Props {
		epubBytes: Uint8Array;
		onActiveSection?: (section: Section) => void;
		onSectionsReady?: (sections: Section[]) => void;
	}
	let { epubBytes, onActiveSection, onSectionsReady }: Props = $props();

	let container: HTMLDivElement | undefined = $state();
	let book: any = null;
	let rendition: any = null;
	let sectionsEmitted = false;

	function reportLocation(location: any) {
		const contents = rendition?.getContents?.() ?? [];
		const text = contents
			.map((item: any) => item?.document?.body?.innerText ?? '')
			.join('\n')
			.trim();
		if (text && onActiveSection) {
			onActiveSection({
				key: `epub:${location?.start?.cfi ?? location?.start?.href ?? 'current'}`,
				label: location?.start?.href ?? 'Current chapter',
				content: text.slice(0, 80_000)
			});
		}
	}

	// Load every chapter once so the whole book becomes section-cached at once
	// instead of chapter-by-chapter as the reader navigates. Skipped in the
	// backend whenever a chapter's snapshot already exists.
	function extractAllChapters() {
		if (!book || !onSectionsReady || sectionsEmitted) return;
		const spine = book.spine?.items ?? book.spine ?? [];
		const sections: Section[] = [];
		let index = 0;
		let fired = false;
		const next = () => {
			const item = spine[index++];
			if (!item) {
				if (!fired && sections.length) {
					fired = true;
					sectionsEmitted = true;
					onSectionsReady(sections);
				}
				return;
			}
			item
				.load(book.load.bind(book))
				.then((doc: any) => {
					const text = (doc?.body?.innerText ?? '').trim().slice(0, 80_000);
					if (text) {
						const href = item.href ?? `chapter-${index}`;
						sections.push({ key: `epub:${href}`, label: href, content: text });
					}
					next();
				})
				.catch(() => next());
		};
		next();
	}

	onMount(() => {
		if (container && epubBytes) {
			book = ePub(epubBytes.buffer as ArrayBuffer);
			rendition = book.renderTo(container, {
				width: '100%',
				height: '100%',
				spread: 'none'
			});
			rendition.on('relocated', reportLocation);
			rendition.display().then(() => {
				reportLocation({ start: { href: 'current' } });
				extractAllChapters();
			});
		}
	});

	onDestroy(() => {
		if (book) {
			book.destroy();
		}
	});
</script>

<div style="width: 100%; height: 100%; display: flex; flex-direction: column;">
	<div
		class="controls"
		style="padding: 8px; display: flex; gap: 8px; justify-content: center; background: var(--bg-panel); border-bottom: 1px solid var(--border-default);"
	>
		<button class="secondary" onclick={() => rendition && rendition.prev()}>Prev</button>
		<button class="secondary" onclick={() => rendition && rendition.next()}>Next</button>
	</div>
	<div bind:this={container} style="flex: 1; overflow: hidden; background: white;"></div>
</div>
