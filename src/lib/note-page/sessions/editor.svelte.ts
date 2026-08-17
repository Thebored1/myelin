import { invoke } from '@tauri-apps/api/core';
import { get } from 'svelte/store';
import { theme } from '$lib/theme';
import type { NoteDocument, SearchResponse } from '$lib/types';
import { vditorI18n } from '$lib/vditorI18n';
import { parseBlocks } from '../model/links';
import type { ControllerContext } from '$lib/controller-context';

/** Owns the lazy Vditor lifecycle and the DOM-only transclusion decoration. */
export function createEditorSession(rawContext: object) {
	const ctx = rawContext as ControllerContext;
	let initGeneration = 0;

	function initVditor() {
		if (!ctx.VditorConstructor || !ctx.vditorContainer || ctx.vditorInstance) return;

		const generation = ++initGeneration;
		const container = ctx.vditorContainer;
		const isCurrentInitialization = () =>
			generation === initGeneration && ctx.vditorContainer === container;

		try {
			const cdn = ctx.localVditorCdn();
			const instance = new ctx.VditorConstructor(container, {
				value: ctx.draftBody,
				cdn,
				_lutePath: `${cdn}/dist/js/lute/lute.min.js`,
				placeholder: ctx.isSourceMaterial ? 'Scratchpad for notes...' : 'Start typing here...',
				mode: 'ir',
				theme: get(theme) === 'light' ? 'classic' : 'dark',
				icon: 'material',
				lang: 'en_US',
				i18n: vditorI18n,
				tab: '\t',
				cache: { enable: false },
				toolbarConfig: { pin: true },
				toolbar: [
					{
						name: 'attach-pdf',
						tipPosition: 'n',
						tip: 'Attach PDF',
						icon: '<svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"></path><polyline points="14 2 14 8 20 8"></polyline></svg>',
						click: () => ctx.openAttachPdfDialog()
					},
					'|',
					'emoji',
					'headings',
					'bold',
					'italic',
					'strike',
					'link',
					'|',
					'list',
					'ordered-list',
					'check',
					'outdent',
					'indent',
					'|',
					'quote',
					'line',
					'code',
					'inline-code',
					'insert-before',
					'insert-after',
					'|',
					{
						name: 'mathlive',
						tipPosition: 'n',
						tip: 'MathLive Editor',
						icon: '<svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round"><path d="M18 4H6l6 8-6 8h12"></path></svg>',
						click: () => void ctx.openMathDialog()
					},
					{
						name: 'link-note',
						tipPosition: 'n',
						tip: 'Link to Note',
						icon: '<svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round"><path d="M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71"></path><path d="M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71"></path></svg>',
						click: () => ctx.openLinkDialog()
					},
					{
						name: 'search-blocks',
						tipPosition: 'n',
						tip: 'Search Global Blocks',
						icon: '<svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round"><circle cx="11" cy="11" r="8"></circle><line x1="21" y1="21" x2="16.65" y2="16.65"></line></svg>',
						click: () => ctx.linkingSession.openGlobalBlockSearch()
					},
					'|',
					'upload',
					'record',
					'table',
					'|',
					'undo',
					'redo',
					'|',
					'fullscreen',
					'edit-mode',
					{
						name: 'more',
						toolbar: ['both', 'code-theme', 'content-theme', 'outline', 'devtools', 'info', 'help']
					}
				],
				after: () => {
					if (!isCurrentInitialization()) return;
					const toolbar = ctx.vditorContainer?.querySelector('.vditor-toolbar');
					if (toolbar) {
						ctx.toolbarResizeObserver = new ResizeObserver(() => {
							ctx.updateToolbarOverflow();
						});
						ctx.toolbarResizeObserver.observe(toolbar);
						ctx.updateToolbarOverflow();

						const fsBtn = toolbar.querySelector('button[data-type="fullscreen"]');
						if (fsBtn) {
							const match = (fsBtn.getAttribute('aria-label') || '').match(/<([^>]+)>/);
							if (match) ctx.fullscreenShortcut = match[1];
						}
					}
					setTimeout(scanForTransclusions, 100);
					setupTransclusionObserver();
				},
				keydown: (e: KeyboardEvent) => {
					if ((e.ctrlKey || e.metaKey) && e.code === 'Comma') {
						e.preventDefault();
						const selector = e.shiftKey
							? 'button[data-type="search-blocks"]'
							: 'button[data-type="link-note"]';
						(ctx.vditorContainer?.querySelector(selector) as HTMLButtonElement | null)?.click();
						return;
					}
					if ((e.ctrlKey || e.metaKey) && e.shiftKey && e.key.toLowerCase() === 'z') {
						e.preventDefault();
						(
							ctx.vditorContainer?.querySelector(
								'button[data-type="redo"]'
							) as HTMLButtonElement | null
						)?.click();
					}
				},
				input: (value: string) => {
					ctx.draftBody = value;
					ctx.triggerAutoSave();
				}
			});
			if (!isCurrentInitialization()) {
				try {
					instance.destroy();
				} catch (error) {
					console.warn('Stale Vditor destroy error:', error);
				}
				return;
			}
			ctx.vditorInstance = instance;
		} catch (e: unknown) {
			if (!isCurrentInitialization()) return;
			ctx.message = 'Vditor Error: ' + (e instanceof Error ? e.message : String(e));
		}
	}

	$effect(() => {
		if (!ctx.toolsReady || !ctx.shouldInitEditor || !ctx.vditorContainer || ctx.vditorInstance)
			return;
		if (!ctx.VditorConstructor && !ctx.vditorLoading) {
			ctx.vditorLoading = true;
			// Vditor's toolbar uses an SVG symbol sprite for its built-in icons.
			// Keep this import inside the lazy editor path so the editor (and its
			// assets) remain lazy-loaded, while ensuring the sprite exists before
			// Vditor renders the toolbar.
			Promise.all([
				import('vditor'),
				import('vditor/dist/index.css'),
				import('vditor/dist/js/icons/material.js')
			])
				.then(([{ default: component }]) => {
					ctx.VditorConstructor = component;
					ctx.vditorLoading = false;
					initVditor();
				})
				.catch((error) => {
					ctx.vditorLoading = false;
					ctx.message = 'Could not load the Markdown editor.';
					console.error('Failed to load Vditor', error);
				});
		} else if (ctx.VditorConstructor) {
			initVditor();
		}
	});

	$effect(() => {
		const skin = get(theme) === 'light' ? 'classic' : 'dark';
		if (ctx.vditorInstance) ctx.vditorInstance.setTheme(skin);
	});

	function scanForTransclusions() {
		if (!ctx.vditorContainer) return;
		const links = ctx.vditorContainer.querySelectorAll(
			'[data-type="a"]:not(.transclusion-wrapper)'
		);
		links.forEach((linkWrapper: Element) => {
			const irLink = linkWrapper.querySelector('.vditor-ir__link');
			if (!irLink) return;
			const blockMatch = (irLink.textContent || '').match(/^\(\(([a-fA-F0-9]{6})\)\)$/);
			if (!blockMatch) return;
			const blockId = blockMatch[1];
			const urlMatch = (linkWrapper.textContent || '').match(
				/\]\(\/notes\/([^#]+)#([a-fA-F0-9]{6})\)$/
			);
			if (!urlMatch) return;
			const targetNoteId = urlMatch[1];
			linkWrapper.classList.add('transclusion-wrapper');
			const cacheKey = `${targetNoteId}#${blockId}`;
			const applyContent = (htmlText: string) => {
				ctx.blockCache[cacheKey] = htmlText;
				const plainText = htmlText.replace(/<[^>]+>/g, '');
				(linkWrapper as HTMLElement).title = plainText;
				(linkWrapper as HTMLElement).setAttribute('data-block-content', plainText);
			};
			if (ctx.blockCache[cacheKey]) {
				applyContent(ctx.blockCache[cacheKey]);
			} else {
				invoke<NoteDocument>('load_note', { noteId: targetNoteId })
					.then((loaded) => {
						const targetBlock = parseBlocks(loaded.body).find((block) => block.id === blockId);
						if (!targetBlock) return;
						let htmlText = targetBlock.original.replace(/\s*\(\([a-fA-F0-9]+\)\)$/, '').trim();
						htmlText = htmlText
							.replace(/\[([^\]]+)\]\(([^)]+)\)/g, '<span class="mock-link">$1</span>')
							.replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>')
							.replace(/\*([^*]+)\*/g, '<em>$1</em>');
						applyContent(htmlText);
					})
					.catch(() => {});
			}
		});
	}

	function setupTransclusionObserver() {
		if (!ctx.vditorContainer) return;
		ctx.transclusionObserver?.disconnect();
		ctx.transclusionObserver = new MutationObserver(scanForTransclusions);
		ctx.transclusionObserver.observe(ctx.vditorContainer, {
			childList: true,
			subtree: true,
			characterData: true
		});
	}

	async function fetchRelatedNotes() {
		if (!ctx.draftTags.trim()) {
			ctx.relatedNotes = [];
			return;
		}
		try {
			const query = ctx.draftTags.split(',')[0].trim();
			if (query) {
				const res = await invoke<SearchResponse>('search_notes', { query });
				ctx.relatedNotes = res.results
					.map((r) => r.note)
					.filter((n) => n.id !== ctx.note?.id)
					.slice(0, 5);
			}
		} catch (e) {
			console.error(e);
		}
	}

	function dispose() {
		invalidateInitialization();
		ctx.toolbarResizeObserver?.disconnect();
		ctx.transclusionObserver?.disconnect();
	}

	function invalidateInitialization() {
		initGeneration += 1;
	}

	return {
		initVditor,
		scanForTransclusions,
		setupTransclusionObserver,
		fetchRelatedNotes,
		dispose,
		invalidateInitialization,
		parseBlocks
	};
}
