<script lang="ts">
	import { onMount, onDestroy } from 'svelte';
	import ePub from 'epubjs';
	import type { Book, Rendition } from 'epubjs';
	import { htmlElementToStructuralText } from '$lib/extraction/structuralText';

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
	type EpubLocation = { start?: { cfi?: string; href?: string } };
	type EpubContents = { document?: Document };
	type EpubSpineItem = {
		href?: string;
		load: (request: (path: string) => Promise<object>) => Document | Promise<Document>;
	};
	type EpubBook = Book & { spine: { items?: EpubSpineItem[] } };
	type EpubRendition = Rendition & {
		getContents: () => EpubContents[];
		on: (event: string, callback: (location: EpubLocation) => void) => void;
	};
	let { epubBytes, onActiveSection, onSectionsReady }: Props = $props();

	let container: HTMLDivElement | undefined = $state();
	let book: EpubBook | null = null;
	let rendition: EpubRendition | null = null;
	let sectionsEmitted = false;

	function reportLocation(location: EpubLocation) {
		const contents = rendition?.getContents?.() ?? [];
		const text = contents
			.map((item: EpubContents) =>
				item?.document?.body ? htmlElementToStructuralText(item.document.body) : ''
			)
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
		const spine = book.spine.items ?? [];
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
			const currentBook = book;
			if (!currentBook) return;
			Promise.resolve(item.load(currentBook.load.bind(currentBook)))
				.then((doc) => {
					const text = doc?.body ? htmlElementToStructuralText(doc.body) : '';
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
			book = ePub(epubBytes.buffer as ArrayBuffer) as EpubBook;
			rendition = book.renderTo(container, {
				width: '100%',
				height: '100%',
				spread: 'none'
			}) as EpubRendition;
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
