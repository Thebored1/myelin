import { invoke } from '@tauri-apps/api/core';
import type { NoteDocument, NoteSummary, SearchResponse } from '$lib/types';
import { parseBlocks } from '../model/links';
import type { BlockItem } from '../types';
import { vditorI18n } from '$lib/vditorI18n';
import type { ControllerContext } from '$lib/controller-context';

type LinkBlockItem = BlockItem & { isFullNote?: boolean };

export function createLinkingSession(rawContext: object) {
	const ctx = rawContext as ControllerContext;
	let linkNoteDialog: HTMLDialogElement | undefined = $state();
	let linkSearchQuery = $state('');
	let linkSearchResults = $state<NoteSummary[]>([]);
	let linkSelectedIndex = $state(0);

	let linkDialogMode = $state<'notes' | 'blocks'>('notes');
	let selectedNoteForBlocks = $state<NoteDocument | null>(null);

	let allNoteBlocks = $state<LinkBlockItem[]>([]);
	const filteredBlocks = $derived(
		linkDialogMode === 'blocks'
			? linkSearchQuery.trim()
				? allNoteBlocks.filter(
						(b) => b.isFullNote || b.text.toLowerCase().includes(linkSearchQuery.toLowerCase())
					)
				: [...allNoteBlocks]
			: []
	);

	let previewNoteDialog: HTMLDialogElement | undefined = $state();
	let previewNoteTarget = $state<NoteDocument | null>(null);
	let previewNoteContainer: HTMLDivElement | undefined = $state();

	$effect(() => {
		const query = linkSearchQuery;
		if (linkDialogMode === 'notes') {
			if (query.trim()) {
				invoke<SearchResponse>('search_notes', { query }).then((res) => {
					linkSearchResults = res.results.map((r) => r.note);
				});
			} else {
				linkSearchResults = [];
			}
		}
	});

	async function openPreviewModal(noteId: string) {
		ctx.isBusy = true;
		try {
			previewNoteTarget = await invoke<NoteDocument>('load_note', { noteId });
			previewNoteDialog?.showModal();
			// Need a tiny delay to ensure previewNoteContainer is bound
			setTimeout(() => {
				if (previewNoteContainer && previewNoteTarget) {
					const cdn = ctx.localVditorCdn();
					ctx.VditorConstructor?.preview(previewNoteContainer, previewNoteTarget.body, {
						mode: 'dark',
						cdn,
						theme: { current: 'dark' },
						i18n: vditorI18n
					});
				}
			}, 50);
		} catch (err) {
			console.error('Failed to load preview note', err);
			alert('Could not load preview.');
		} finally {
			ctx.isBusy = false;
		}
	}

	async function handleVditorClick(e: MouseEvent) {
		const target = e.target as HTMLElement;

		let href = '';

		// 1. Standard HTML links (WYSIWYG or preview modes)
		const link = target.closest('a');
		if (link) {
			href = link.getAttribute('href') || '';
		}

		// 2. Vditor Instant Rendering (IR) mode links
		if (!href) {
			const irLink = target.closest('[data-type="a"]');
			if (irLink) {
				const text = irLink.textContent || '';
				// IR links look like [text](/notes/targetId)
				const match = text.match(/\]\(([^)]+)\)/);
				if (match && match[1]) {
					href = match[1].trim();
				}
			}
		}

		if (!href) return;

		if (href.startsWith('/notes/')) {
			e.preventDefault();
			e.stopPropagation();
			const fullTargetId = decodeURIComponent(href.replace('/notes/', ''));
			const targetId = fullTargetId.split('#')[0];
			await openPreviewModal(targetId);
		}
	}

	function handleVditorKeydownCapture(e: KeyboardEvent) {
		// Prevent WYSIWYG mode shortcut (Cmd/Ctrl + Alt + 7)
		if ((e.ctrlKey || e.metaKey) && e.altKey && !e.shiftKey && e.code === 'Digit7') {
			e.preventDefault();
			e.stopPropagation();
		}

		// Prevent Ctrl+Arrow keys (Up/Down) from scrolling in the editor, but allow Shift for text selection
		if ((e.ctrlKey || e.metaKey) && !e.shiftKey && (e.key === 'ArrowUp' || e.key === 'ArrowDown')) {
			e.preventDefault();
			e.stopPropagation();
		}

		// Vditor has a bug where it freezes during Shift+Arrow selection across nodes.
		// By completely stopping propagation, the browser's native text selection engine
		// takes over flawlessly and Vditor's internal range parser never runs.
		if (e.shiftKey && e.key.startsWith('Arrow')) {
			e.stopImmediatePropagation();
		}
	}

	function handleVditorKeyupCapture(e: KeyboardEvent) {
		// Stop Vditor's keyup processor (which calls expandMarker and freezes)
		if (e.shiftKey && e.key.startsWith('Arrow')) {
			e.stopImmediatePropagation();
		}
	}

	function handleLinkSearchKeydown(e: KeyboardEvent) {
		const targetListLength =
			linkDialogMode === 'notes' ? linkSearchResults.length : filteredBlocks.length;
		if (e.key === 'ArrowDown') {
			e.preventDefault();
			linkSelectedIndex = Math.min(targetListLength - 1, linkSelectedIndex + 1);
		} else if (e.key === 'ArrowUp') {
			e.preventDefault();
			linkSelectedIndex = Math.max(0, linkSelectedIndex - 1);
		} else if (e.key === 'Enter') {
			e.preventDefault();
			if (targetListLength > 0) {
				if (linkDialogMode === 'notes') {
					selectNoteForBlocks(linkSearchResults[linkSelectedIndex]);
				} else {
					insertBlockLink(filteredBlocks[linkSelectedIndex]);
				}
			}
		}
	}

	function autofocus(node: HTMLElement) {
		node.focus();
	}

	async function selectNoteForBlocks(target: NoteSummary) {
		ctx.isBusy = true;
		try {
			selectedNoteForBlocks = await invoke<NoteDocument>('load_note', { noteId: target.id });
			allNoteBlocks = [
				{ text: `Link to entire note: ${target.title}`, id: null, original: '', isFullNote: true },
				...parseBlocks(selectedNoteForBlocks.body)
			];
			linkSearchQuery = '';
			linkDialogMode = 'blocks';
			linkSelectedIndex = 0;
		} catch (e) {
			console.error('Failed to load note for blocks', e);
		} finally {
			ctx.isBusy = false;
		}
	}

	async function insertBlockLink(block: LinkBlockItem) {
		if (!selectedNoteForBlocks) return;

		if (block.isFullNote) {
			ctx.shouldRefocusEditor = true;
			linkNoteDialog?.close();
			const linkText = `[${selectedNoteForBlocks.title}](/notes/${selectedNoteForBlocks.id}) `;
			ctx.insertAtSavedCursor(linkText);
			ctx.refocusEditorSoon();
			return;
		}

		let blockId = block.id;
		if (!blockId) {
			blockId = Math.random().toString(16).substring(2, 8);
			const newBlockText = `${block.original} ((${blockId}))`;
			selectedNoteForBlocks.body = selectedNoteForBlocks.body.replace(block.original, newBlockText);
			await invoke('save_note', {
				noteId: selectedNoteForBlocks.id,
				title: selectedNoteForBlocks.title,
				tags: selectedNoteForBlocks.tags,
				body: selectedNoteForBlocks.body,
				sourcePdf: selectedNoteForBlocks.sourcePdf,
				annotations: selectedNoteForBlocks.annotations
			});

			if (selectedNoteForBlocks.id === ctx.note?.id) {
				setTimeout(() => {
					if (ctx.vditorInstance) {
						const editorEl = ctx.vditorContainer?.querySelector('.vditor-ir') as HTMLElement | null;
						const selectionOffset = editorEl ? ctx.getSelectionTextOffset(editorEl) : null;
						let currentBody = ctx.vditorInstance.getValue();
						if (!currentBody.includes(block.original)) {
							const escaped = block.original.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
							const regex = new RegExp(escaped.replace(/\s+/g, '\\s+'));
							currentBody = currentBody.replace(regex, `$& ((${blockId}))`);
						} else {
							currentBody = currentBody.replace(block.original, newBlockText);
						}
						ctx.vditorInstance.setValue(currentBody);
						ctx.draftBody = currentBody;
						if (selectionOffset !== null) {
							setTimeout(() => {
								ctx.focusEditor();
								const refreshedEditorEl = ctx.vditorContainer?.querySelector(
									'.vditor-ir'
								) as HTMLElement | null;
								if (refreshedEditorEl)
									ctx.restoreSelectionTextOffset(refreshedEditorEl, selectionOffset);
							}, 0);
						}
					}
				}, 50);
			}
		}

		ctx.shouldRefocusEditor = true;
		linkNoteDialog?.close();
		const linkText = `[((${blockId}))](/notes/${selectedNoteForBlocks!.id}#${blockId}) `;
		ctx.insertAtSavedCursor(linkText);
		ctx.refocusEditorSoon();
	}

	let globalSearchDialog: HTMLDialogElement | undefined = $state();
	let globalSearchQuery = $state('');
	let globalSelectedIndex = $state(0);

	let globalBlocks = $state<LinkBlockItem[]>([]);
	const filteredGlobalBlocks = $derived(
		globalSearchQuery.trim()
			? globalBlocks.filter((b) => b.text.toLowerCase().includes(globalSearchQuery.toLowerCase()))
			: globalBlocks.slice(0, 50)
	);

	async function openGlobalBlockSearch() {
		ctx.saveCursorPosition();
		globalSearchQuery = '';
		globalSelectedIndex = 0;
		globalSearchDialog?.showModal();
		setTimeout(() => {
			const input = globalSearchDialog?.querySelector('.link-search-input') as HTMLInputElement;
			if (input) input.focus();
		}, 50);

		ctx.isBusy = true;
		try {
			const docs = await invoke<NoteDocument[]>('get_all_note_documents');
			const allBlocks: BlockItem[] = [];
			for (const doc of docs) {
				const blocks = parseBlocks(doc.body);
				for (const b of blocks) {
					b.sourceNoteId = doc.id;
					b.sourceNoteTitle = doc.title;
					allBlocks.push(b);
				}
			}
			globalBlocks = allBlocks;
		} catch (err) {
			console.error('Failed to load global blocks', err);
		} finally {
			ctx.isBusy = false;
		}
	}

	function handleGlobalSearchKeydown(e: KeyboardEvent) {
		const targetListLength = filteredGlobalBlocks.length;
		if (e.key === 'ArrowDown') {
			e.preventDefault();
			globalSelectedIndex = Math.min(targetListLength - 1, globalSelectedIndex + 1);
		} else if (e.key === 'ArrowUp') {
			e.preventDefault();
			globalSelectedIndex = Math.max(0, globalSelectedIndex - 1);
		} else if (e.key === 'Enter') {
			e.preventDefault();
			if (targetListLength > 0) {
				void insertGlobalBlockLink(filteredGlobalBlocks[globalSelectedIndex]);
			}
		}
	}

	async function insertGlobalBlockLink(block: LinkBlockItem) {
		if (!block.sourceNoteId || !block.sourceNoteTitle) return;

		let blockId = block.id;
		const isNewBlock = !blockId;
		if (isNewBlock) {
			blockId = Math.random().toString(16).substring(2, 8);
		}

		ctx.shouldRefocusEditor = true;
		globalSearchDialog?.close();
		const linkText = `[((${blockId}))](/notes/${block.sourceNoteId}#${blockId}) `;

		if (isNewBlock) {
			const newBlockText = `${block.original} ((${blockId}))`;
			ctx.isBusy = true;
			try {
				const sourceDoc = await invoke<NoteDocument>('load_note', { noteId: block.sourceNoteId });
				sourceDoc.body = sourceDoc.body.replace(block.original, newBlockText);
				await invoke('save_note', {
					noteId: sourceDoc.id,
					title: sourceDoc.title,
					tags: sourceDoc.tags,
					body: sourceDoc.body,
					sourcePdf: sourceDoc.sourcePdf,
					annotations: sourceDoc.annotations
				});

				if (sourceDoc.id === ctx.note?.id) {
					setTimeout(() => {
						if (ctx.vditorInstance) {
							const editorEl = ctx.vditorContainer?.querySelector(
								'.vditor-ir'
							) as HTMLElement | null;
							const selectionOffset = editorEl ? ctx.getSelectionTextOffset(editorEl) : null;
							let currentBody = ctx.vditorInstance.getValue();
							if (!currentBody.includes(block.original)) {
								const escaped = block.original.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
								const regex = new RegExp(escaped.replace(/\s+/g, '\\s+'));
								currentBody = currentBody.replace(regex, `$& ((${blockId}))`);
							} else {
								currentBody = currentBody.replace(block.original, newBlockText);
							}
							ctx.vditorInstance.setValue(currentBody);
							ctx.draftBody = currentBody;
							if (selectionOffset !== null) {
								setTimeout(() => {
									ctx.focusEditor();
									const refreshedEditorEl = ctx.vditorContainer?.querySelector(
										'.vditor-ir'
									) as HTMLElement | null;
									if (refreshedEditorEl)
										ctx.restoreSelectionTextOffset(refreshedEditorEl, selectionOffset);
								}, 0);
							}
						}
					}, 50);
				}

				ctx.insertAtSavedCursor(linkText);
			} catch (err) {
				console.error('Failed to append block ID to source note', err);
				ctx.message = 'Failed to update source note.';
				setTimeout(() => (ctx.message = ''), 3000);
			} finally {
				ctx.isBusy = false;
				ctx.refocusEditorSoon();
			}
		} else {
			ctx.insertAtSavedCursor(linkText);
			ctx.refocusEditorSoon();
		}
	}

	return {
		get linkNoteDialog() {
			return linkNoteDialog;
		},
		set linkNoteDialog(value) {
			linkNoteDialog = value;
		},
		get linkSearchQuery() {
			return linkSearchQuery;
		},
		set linkSearchQuery(value) {
			linkSearchQuery = value;
		},
		get linkSearchResults() {
			return linkSearchResults;
		},
		set linkSearchResults(value) {
			linkSearchResults = value;
		},
		get linkSelectedIndex() {
			return linkSelectedIndex;
		},
		set linkSelectedIndex(value) {
			linkSelectedIndex = value;
		},
		get linkDialogMode() {
			return linkDialogMode;
		},
		set linkDialogMode(value) {
			linkDialogMode = value;
		},
		get selectedNoteForBlocks() {
			return selectedNoteForBlocks;
		},
		set selectedNoteForBlocks(value) {
			selectedNoteForBlocks = value;
		},
		get allNoteBlocks() {
			return allNoteBlocks;
		},
		set allNoteBlocks(value) {
			allNoteBlocks = value;
		},
		get filteredBlocks() {
			return filteredBlocks;
		},
		get previewNoteDialog() {
			return previewNoteDialog;
		},
		set previewNoteDialog(value) {
			previewNoteDialog = value;
		},
		get previewNoteTarget() {
			return previewNoteTarget;
		},
		set previewNoteTarget(value) {
			previewNoteTarget = value;
		},
		get previewNoteContainer() {
			return previewNoteContainer;
		},
		set previewNoteContainer(value) {
			previewNoteContainer = value;
		},
		get globalSearchDialog() {
			return globalSearchDialog;
		},
		set globalSearchDialog(value) {
			globalSearchDialog = value;
		},
		get globalSearchQuery() {
			return globalSearchQuery;
		},
		set globalSearchQuery(value) {
			globalSearchQuery = value;
		},
		get globalSelectedIndex() {
			return globalSelectedIndex;
		},
		set globalSelectedIndex(value) {
			globalSelectedIndex = value;
		},
		get globalBlocks() {
			return globalBlocks;
		},
		set globalBlocks(value) {
			globalBlocks = value;
		},
		get filteredGlobalBlocks() {
			return filteredGlobalBlocks;
		},
		openPreviewModal,
		handleVditorClick,
		handleVditorKeydownCapture,
		handleVditorKeyupCapture,
		handleLinkSearchKeydown,
		autofocus,
		selectNoteForBlocks,
		insertBlockLink,
		openGlobalBlockSearch,
		handleGlobalSearchKeydown,
		insertGlobalBlockLink
	};
}
