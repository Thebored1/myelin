<script lang="ts">
	import { invoke } from '@tauri-apps/api/core';
	import { listen, type UnlistenFn } from '@tauri-apps/api/event';
	import { goto, beforeNavigate } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import type { NoteDocument, SearchResponse, NoteSummary, PdfAnnotation, GitCommit, ChatMessage } from '$lib/types';
	import { onMount, onDestroy } from 'svelte';
	import { sidebarOpen, showSidebarToggle } from '$lib/stores';
	import Vditor from 'vditor';
	import 'vditor/dist/index.css';
	import 'mathlive';
	import 'mathlive/fonts.css';
	import PdfViewer from '$lib/components/PdfViewer.svelte';

	let note = $state<NoteDocument | null>(null);
	let draftBody = $state('');
	let draftTitle = $state('');
	let draftTags = $state('');
	let isBusy = $state(false);
	let message = $state('');
	
	let activeSidebarTab = $state<'info'|'chat'|'versions'>('info');
	let noteHistory = $state<GitCommit[]>([]);
	let versionPreviewContent = $state<string | null>(null);
	let versionPreviewHash = $state<string | null>(null);
	let versionPreviewDialog: HTMLDialogElement | undefined = $state();
	let chatMessages = $state<ChatMessage[]>([]);
	let chatInput = $state('');
	let chatTextareaEl: HTMLTextAreaElement | undefined = $state();

	let backUrl = $derived(page.url.searchParams.get('returnTo') || '/');
	
	let relatedNotes = $state<NoteSummary[]>([]);
	let vditorContainer: HTMLElement | undefined = $state();
	let vditorInstance: Vditor | null = null;
	let fullscreenShortcut = $state('Esc');
	let savedEditorRange: Range | null = null;
	let shouldRefocusEditor = false;

	let isMainNotePdf = $state(false);
	let activePdfId = $state<string | null>(null);
	let activePdfBytes = $state<Uint8Array | null>(null);
	let scratchpadSavedId = $state<string | null>(null);
	let showAttachedNote = $state(false);
	
	let splitRatio = $state(50);
	let isResizing = $state(false);
	let mainLayoutEl: HTMLElement | undefined = $state();

	let sidebarWidth = $state(320);
	let isSidebarResizing = $state(false);

	function startSidebarResizing(e: MouseEvent) {
		e.preventDefault();
		isSidebarResizing = true;
	}

	function startResizing() {
		isResizing = true;
	}

	function handleGlobalMouseMove(e: MouseEvent) {
		if (isResizing && mainLayoutEl) {
			const rect = mainLayoutEl.getBoundingClientRect();
			const newRatio = ((e.clientX - rect.left) / rect.width) * 100;
			if (newRatio > 20 && newRatio < 80) {
				splitRatio = newRatio;
			}
		} else if (isSidebarResizing) {
			const newWidth = window.innerWidth - e.clientX;
			sidebarWidth = Math.max(320, Math.min(newWidth, window.innerWidth * 0.8));
		}
	}

	function stopResizing() {
		if (isResizing || isSidebarResizing) {
			isResizing = false;
			isSidebarResizing = false;
			if (vditorInstance) {
				// Let Vditor resize after layout shift
				setTimeout(() => {
					window.dispatchEvent(new Event('resize'));
				}, 50);
			}
		}
	}

	function handlePdfQuote(text: string, page: number) {
		appendToNoteBody(`\n> ${text}\n> *(Page ${page})*\n\n`);
	}

	function focusEditor() {
		if (!vditorInstance || !vditorContainer) return;
		vditorInstance.focus();
		const editorEl = vditorContainer.querySelector('.vditor-ir') as HTMLElement | null;
		editorEl?.focus();
	}

	function refocusEditorSoon() {
		shouldRefocusEditor = false;
		setTimeout(() => {
			focusEditor();
		}, 0);
	}

	function getSelectionTextOffset(editorEl: HTMLElement): number | null {
		const selection = window.getSelection();
		if (!selection || selection.rangeCount === 0) return null;

		const range = selection.getRangeAt(0);
		if (!editorEl.contains(range.endContainer)) return null;

		const walker = document.createTreeWalker(editorEl, NodeFilter.SHOW_TEXT);
		let offset = 0;
		let node: Node | null;
		while ((node = walker.nextNode())) {
			const textLength = node.textContent?.length ?? 0;
			if (node === range.endContainer) {
				return offset + range.endOffset;
			}
			offset += textLength;
		}

		return offset;
	}

	function restoreSelectionTextOffset(editorEl: HTMLElement, targetOffset: number) {
		const selection = window.getSelection();
		if (!selection) return;

		const walker = document.createTreeWalker(editorEl, NodeFilter.SHOW_TEXT);
		let offset = 0;
		let node: Node | null;
		while ((node = walker.nextNode())) {
			const textLength = node.textContent?.length ?? 0;
			const nextOffset = offset + textLength;
			if (targetOffset <= nextOffset) {
				const range = document.createRange();
				range.setStart(node, Math.max(0, targetOffset - offset));
				range.collapse(true);
				selection.removeAllRanges();
				selection.addRange(range);
				return;
			}
			offset = nextOffset;
		}

		editorEl.focus();
	}
	
	function saveCursorPosition() {
		if (!vditorInstance || !vditorContainer) return;
		const editorEl = vditorContainer.querySelector('.vditor-ir') as HTMLElement | null;
		const selection = window.getSelection();
		if (!editorEl || !selection || selection.rangeCount === 0) return;

		const range = selection.getRangeAt(0);
		if (!editorEl.contains(range.commonAncestorContainer)) return;

		savedEditorRange = range.cloneRange();
	}
	
	function insertAtSavedCursor(linkText: string) {
		if (!vditorInstance || !vditorContainer) return;
		const editorEl = vditorContainer.querySelector('.vditor-ir') as HTMLElement | null;
		if (!editorEl) return;
		
		focusEditor();

		const selection = window.getSelection();
		if (savedEditorRange && selection) {
			selection.removeAllRanges();
			selection.addRange(savedEditorRange);
		}

		const inserted = document.execCommand('insertText', false, linkText);
		if (!inserted) {
			vditorInstance.insertValue(linkText, true);
		}
		
		savedEditorRange = null;
		focusEditor();
		draftBody = vditorInstance.getValue();
		triggerAutoSave();
	}
	
	let mathDialog: HTMLDialogElement | undefined = $state();
	let mathValue = $state('');

	let linkNoteDialog: HTMLDialogElement | undefined = $state();
	let linkSearchQuery = $state('');
	let linkSearchResults = $state<NoteSummary[]>([]);
	let linkSelectedIndex = $state(0);
	
	let linkDialogMode = $state<'notes' | 'blocks'>('notes');
	let selectedNoteForBlocks = $state<NoteDocument | null>(null);
	
	type BlockItem = { text: string, id: string | null, original: string, isFullNote?: boolean, sourceNoteId?: string, sourceNoteTitle?: string };
	let allNoteBlocks = $state<BlockItem[]>([]);
	let filteredBlocks = $derived(
		linkDialogMode === 'blocks'
			? (linkSearchQuery.trim() 
				? allNoteBlocks.filter(b => b.isFullNote || b.text.toLowerCase().includes(linkSearchQuery.toLowerCase())) 
				: [...allNoteBlocks])
			: []
	);
	
	let previewNoteDialog: HTMLDialogElement | undefined = $state();
	let previewNoteTarget = $state<NoteDocument | null>(null);
	let previewNoteContainer: HTMLDivElement | undefined = $state();
	
	let blockCache: Record<string, string> = {};
	let transclusionObserver: MutationObserver | null = null;
	
	let toolbarExpanded = $state(false);
	let toolbarNeedsToggle = $state(false);
	let toolbarResizeObserver: ResizeObserver | null = null;
	
	let saveStatus = $state<'saved' | 'saving' | 'unsaved'>('saved');
	let saveTimer: ReturnType<typeof setTimeout> | null = null;
	let navigationWarningDialog: HTMLDialogElement | undefined = $state();
	let deleteAttachedNoteDialog: HTMLDialogElement | undefined = $state();
	let pendingNavigationUrl = $state('');
	let shouldRenderEditor = $derived(note !== null && (!isMainNotePdf || showAttachedNote));
	let shouldInitEditor = $derived(note !== null && (!isMainNotePdf || showAttachedNote));
	let loadedRouteNoteId = $state('');

	function appendToNoteBody(content: string) {
		showAttachedNote = true;
		if (vditorInstance) {
			vditorInstance.insertValue(content);
			draftBody = vditorInstance.getValue();
		} else {
			draftBody = `${draftBody}${content}`;
		}
		triggerAutoSave();
	}

	function destroyEditorInstance() {
		if (!vditorInstance) return;
		try {
			vditorInstance.destroy();
		} catch (e) {
			console.warn("Vditor destroy error:", e);
		}
		vditorInstance = null;
	}

	function triggerAutoSave() {
		if (saveStatus !== 'saving') saveStatus = 'unsaved';
		if (saveTimer) clearTimeout(saveTimer);
		saveTimer = setTimeout(() => {
			void saveNote();
		}, 1000);
	}

	function insertMath() {
		if (vditorInstance && mathValue) {
			// Replace MathLive specific placeholders with standard KaTeX squares
			const cleanMath = mathValue.replace(/\\(?:_)?placeholder(?:\[.*?\])?(?:{})?/g, '\\square');
			vditorInstance.insertValue(`\n$$\n${cleanMath}\n$$\n`);
		}
		mathDialog?.close();
	}

	$effect(() => {
		const query = linkSearchQuery;
		if (linkDialogMode === 'notes') {
			if (query.trim()) {
				invoke<SearchResponse>('search_notes', { query }).then(res => {
					linkSearchResults = res.results.map(r => r.note);
				});
			} else {
				linkSearchResults = [];
			}
		}
	});

	async function openPreviewModal(noteId: string) {
		isBusy = true;
		try {
			previewNoteTarget = await invoke<NoteDocument>('load_note', { noteId });
			previewNoteDialog?.showModal();
			// Need a tiny delay to ensure previewNoteContainer is bound
			setTimeout(() => {
				if (previewNoteContainer && previewNoteTarget) {
					Vditor.preview(previewNoteContainer, previewNoteTarget.body, {
						mode: 'dark',
						theme: { current: 'dark' }
					});
				}
			}, 50);
		} catch (err) {
			console.error("Failed to load preview note", err);
			alert("Could not load preview.");
		} finally {
			isBusy = false;
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
		if ((e.ctrlKey || e.metaKey) && e.altKey && !e.shiftKey && e.code === "Digit7") {
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
		const targetListLength = linkDialogMode === 'notes' ? linkSearchResults.length : filteredBlocks.length;
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

	function parseBlocks(markdown: string): BlockItem[] {
		const chunks = markdown.split(/\n+/);
		return chunks.map(chunk => {
			const text = chunk.trim();
			if (!text) return null;
			const idMatch = text.match(/\(\(([a-fA-F0-9]{6})\)\)$/);
			
			let cleanDisplay = text.replace(/\s*\(\([a-fA-F0-9]{6}\)\)$/, '');
			cleanDisplay = cleanDisplay.replace(/\[([^\]]+)\]\([^)]+\)/g, '$1');
			cleanDisplay = cleanDisplay.replace(/(\*\*|__)(.*?)\1/g, '$2');
			cleanDisplay = cleanDisplay.replace(/(\*|_)(.*?)\1/g, '$2');
			cleanDisplay = cleanDisplay.replace(/^#+\s+/g, '');
			
			return {
				text: cleanDisplay,
				id: idMatch ? idMatch[1] : null,
				original: text
			};
		}).filter(Boolean) as BlockItem[];
	}

	async function selectNoteForBlocks(target: NoteSummary) {
		isBusy = true;
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
			console.error("Failed to load note for blocks", e);
		} finally {
			isBusy = false;
		}
	}

	async function insertBlockLink(block: BlockItem) {
		if (!selectedNoteForBlocks) return;
		
		if (block.isFullNote) {
			shouldRefocusEditor = true;
			linkNoteDialog?.close();
			const linkText = `[${selectedNoteForBlocks.title}](/notes/${selectedNoteForBlocks.id}) `;
			insertAtSavedCursor(linkText);
			refocusEditorSoon();
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
			
			if (selectedNoteForBlocks.id === note?.id) {
				setTimeout(() => {
					if (vditorInstance) {
						const editorEl = vditorContainer?.querySelector('.vditor-ir') as HTMLElement | null;
						const selectionOffset = editorEl ? getSelectionTextOffset(editorEl) : null;
						let currentBody = vditorInstance.getValue();
						if (!currentBody.includes(block.original)) {
							const escaped = block.original.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
							const regex = new RegExp(escaped.replace(/\s+/g, '\\s+'));
							currentBody = currentBody.replace(regex, `$& ((${blockId}))`);
						} else {
							currentBody = currentBody.replace(block.original, newBlockText);
						}
						vditorInstance.setValue(currentBody);
						draftBody = currentBody;
						if (selectionOffset !== null) {
							setTimeout(() => {
								focusEditor();
								const refreshedEditorEl = vditorContainer?.querySelector('.vditor-ir') as HTMLElement | null;
								if (refreshedEditorEl) restoreSelectionTextOffset(refreshedEditorEl, selectionOffset);
							}, 0);
						}
					}
				}, 50);
			}
		}
		
		shouldRefocusEditor = true;
		linkNoteDialog?.close();
		const linkText = `[((${blockId}))](/notes/${selectedNoteForBlocks!.id}#${blockId}) `;
		insertAtSavedCursor(linkText);
		refocusEditorSoon();
	}

	let globalSearchDialog: HTMLDialogElement | undefined = $state();
	let globalSearchQuery = $state('');
	let globalSelectedIndex = $state(0);
	
	let globalBlocks = $state<BlockItem[]>([]);
	let filteredGlobalBlocks = $derived(
		globalSearchQuery.trim()
			? globalBlocks.filter(b => b.text.toLowerCase().includes(globalSearchQuery.toLowerCase()))
			: globalBlocks.slice(0, 50)
	);

	async function openGlobalBlockSearch() {
		saveCursorPosition();
		globalSearchQuery = '';
		globalSelectedIndex = 0;
		globalSearchDialog?.showModal();
		setTimeout(() => {
			const input = globalSearchDialog?.querySelector('.link-search-input') as HTMLInputElement;
			if (input) input.focus();
		}, 50);
		
		isBusy = true;
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
			console.error("Failed to load global blocks", err);
		} finally {
			isBusy = false;
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

	async function insertGlobalBlockLink(block: BlockItem) {
		if (!block.sourceNoteId || !block.sourceNoteTitle) return;
		
		let blockId = block.id;
		const isNewBlock = !blockId;
		if (isNewBlock) {
			blockId = Math.random().toString(16).substring(2, 8);
		}
		
		shouldRefocusEditor = true;
		globalSearchDialog?.close();
		const linkText = `[((${blockId}))](/notes/${block.sourceNoteId}#${blockId}) `;

		if (isNewBlock) {
			const newBlockText = `${block.original} ((${blockId}))`;
			isBusy = true;
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

				if (sourceDoc.id === note?.id) {
					setTimeout(() => {
						if (vditorInstance) {
							const editorEl = vditorContainer?.querySelector('.vditor-ir') as HTMLElement | null;
							const selectionOffset = editorEl ? getSelectionTextOffset(editorEl) : null;
							let currentBody = vditorInstance.getValue();
							if (!currentBody.includes(block.original)) {
								const escaped = block.original.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
								const regex = new RegExp(escaped.replace(/\s+/g, '\\s+'));
								currentBody = currentBody.replace(regex, `$& ((${blockId}))`);
							} else {
								currentBody = currentBody.replace(block.original, newBlockText);
							}
							vditorInstance.setValue(currentBody);
							draftBody = currentBody;
							if (selectionOffset !== null) {
								setTimeout(() => {
									focusEditor();
									const refreshedEditorEl = vditorContainer?.querySelector('.vditor-ir') as HTMLElement | null;
									if (refreshedEditorEl) restoreSelectionTextOffset(refreshedEditorEl, selectionOffset);
								}, 0);
							}
						}
					}, 50);
				}

				insertAtSavedCursor(linkText);
			} catch (err) {
				console.error("Failed to append block ID to source note", err);
				message = "Failed to update source note.";
				setTimeout(() => message = '', 3000);
			} finally {
				isBusy = false;
				refocusEditorSoon();
			}
		} else {
			insertAtSavedCursor(linkText);
			refocusEditorSoon();
		}
	}

	async function loadCurrentNote(noteId: string) {
		destroyEditorInstance();
		activePdfBytes = null;
		activePdfId = null;
		showAttachedNote = false;
		note = await invoke<NoteDocument>('load_note', { noteId });
		const loadedNote = note;
		chatMessages = loadedNote.chatHistory || [];
		noteHistory = [];
		versionPreviewContent = null;
		activeSidebarTab = 'info';

		isMainNotePdf = loadedNote.relativePath.toLowerCase().endsWith('.pdf');
		if (isMainNotePdf) {
			const allNotes = await invoke<NoteDocument[]>('get_all_note_documents');
			const existingScratchpad =
				allNotes
					.filter((candidate) => candidate.sourcePdf === loadedNote.id)
					.sort((left, right) => right.updatedAt.localeCompare(left.updatedAt))[0] ?? null;
			draftTitle = loadedNote.title;
			draftBody = existingScratchpad?.body ?? "";
			draftTags = loadedNote.tags.join(', ');
			activePdfId = loadedNote.id;
			const bytes = await invoke<number[]>('read_pdf_binary', { noteId: loadedNote.id });
			activePdfBytes = new Uint8Array(bytes);
			scratchpadSavedId = existingScratchpad?.id ?? null;
			showAttachedNote = draftBody.trim().length > 0;
		} else {
			draftTitle = loadedNote.title;
			draftBody = loadedNote.body;
			draftTags = loadedNote.tags.join(', ');
			
			if (loadedNote.sourcePdf) {
				activePdfId = loadedNote.sourcePdf;
				const bytes = await invoke<number[]>('read_pdf_binary', { noteId: loadedNote.sourcePdf });
				activePdfBytes = new Uint8Array(bytes);
				showAttachedNote = draftBody.trim().length > 0;
			} else {
				activePdfId = null;
				activePdfBytes = null;
				showAttachedNote = true;
			}
		}

		message = '';
		void fetchRelatedNotes();
		
	}

	function initVditor() {
		if (!vditorContainer || vditorInstance) return;
		
		try {
			vditorInstance = new Vditor(vditorContainer, {
			value: draftBody,
			placeholder: isMainNotePdf ? "Scratchpad for PDF notes..." : "Start typing here...",
			mode: 'ir',
			theme: 'dark',
			icon: 'material',
			lang: 'en_US',
			tab: '\t',
			cache: { enable: false },
			toolbarConfig: { pin: true },
			toolbar: [
				"emoji", "headings", "bold", "italic", "strike", "link", "|",
				"list", "ordered-list", "check", "outdent", "indent", "|",
				"quote", "line", "code", "inline-code", "insert-before", "insert-after", "|",
				{
					name: 'mathlive',
					tipPosition: 'n',
					tip: 'MathLive Editor',
					icon: '<svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round"><path d="M18 4H6l6 8-6 8h12"></path></svg>',
					click: () => {
						mathValue = '';
						mathDialog?.showModal();
					}
				},
				{
					name: 'link-note',
					tipPosition: 'n',
					tip: 'Link to Note',
					icon: '<svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round"><path d="M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71"></path><path d="M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71"></path></svg>',
					click: () => {
						saveCursorPosition();
						linkSearchQuery = '';
						linkSearchResults = [];
						linkNoteDialog?.showModal();
						setTimeout(() => {
							const input = linkNoteDialog?.querySelector('.link-search-input') as HTMLInputElement;
							if (input) input.focus();
						}, 50);
					}
				},
				{
					name: 'search-blocks',
					tipPosition: 'n',
					tip: 'Search Global Blocks',
					icon: '<svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round"><circle cx="11" cy="11" r="8"></circle><line x1="21" y1="21" x2="16.65" y2="16.65"></line></svg>',
					click: () => {
						openGlobalBlockSearch();
					}
				},
				"|",
				"upload", "record", "table", "|", "undo", "redo", "|", "fullscreen", "edit-mode",
				{
					name: "more",
					toolbar: [
						"both", "code-theme", "content-theme", "outline", "devtools", "info", "help"
					]
				}
			],
			after: () => {
				const toolbar = vditorContainer?.querySelector('.vditor-toolbar');
				if (toolbar) {
					toolbarResizeObserver = new ResizeObserver(() => {
						if (toolbar.scrollHeight > 55) {
							toolbarNeedsToggle = true;
						} else {
							toolbarNeedsToggle = false;
							toolbarExpanded = false;
						}
						updateToolbarOverflow();
					});
					toolbarResizeObserver.observe(toolbar);
					if (toolbar.scrollHeight > 55) {
						toolbarNeedsToggle = true;
					}
					updateToolbarOverflow();
					
					const fsBtn = toolbar.querySelector('button[data-type="fullscreen"]');
					if (fsBtn) {
						const label = fsBtn.getAttribute('aria-label') || '';
						const match = label.match(/<([^>]+)>/);
						if (match) {
							fullscreenShortcut = match[1];
						}
					}
				}
				setTimeout(() => {
					scanForTransclusions();
				}, 100);
				setupTransclusionObserver();
			},
			keydown: (e: KeyboardEvent) => {
				if ((e.ctrlKey || e.metaKey) && e.code === 'Comma') {
					e.preventDefault();
					if (e.shiftKey) {
						const globalSearchBtn = vditorContainer?.querySelector('button[data-type="search-blocks"]') as HTMLButtonElement | null;
						if (globalSearchBtn) globalSearchBtn.click();
					} else {
						const linkBtn = vditorContainer?.querySelector('button[data-type="link-note"]') as HTMLButtonElement | null;
						if (linkBtn) linkBtn.click();
					}
					return;
				}
				if ((e.ctrlKey || e.metaKey) && e.shiftKey && e.key.toLowerCase() === 'z') {
					e.preventDefault();
					const redoBtn = vditorContainer?.querySelector('button[data-type="redo"]') as HTMLButtonElement | null;
					if (redoBtn) redoBtn.click();
				}
			},
			input: (value) => {
				draftBody = value;
				triggerAutoSave();
			}
		});
		} catch (e: any) {
			message = "Vditor Error: " + (e?.message || String(e));
		}
	}

	$effect(() => {
		if (shouldInitEditor && vditorContainer && !vditorInstance) {
			initVditor();
		}
	});

	function parseBacklinkContext(context: string): string {
		if (!context) return '';
		let html = context;
		// Strip markdown links but keep text and make it look like a link
		html = html.replace(/\[([^\]]+)\]\([^)]+\)/g, '<span style="color: var(--accent-200); font-weight: 500;">$1</span>');
		// Strip transclusion syntax
		html = html.replace(/\(\([a-fA-F0-9]{6}\)\)/g, '<span style="color: var(--text-secondary);">(Block Link)</span>');
		// Bold and italic
		html = html.replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>');
		html = html.replace(/\*([^*]+)\*/g, '<em>$1</em>');
		return html;
	}

	function scanForTransclusions() {
		if (!vditorContainer) return;
		const links = vditorContainer.querySelectorAll('[data-type="a"]:not(.transclusion-wrapper)');
		links.forEach(linkWrapper => {
			const irLink = linkWrapper.querySelector('.vditor-ir__link');
			if (!irLink) return;
			const text = irLink.textContent || '';
			const blockMatch = text.match(/^\(\(([a-fA-F0-9]{6})\)\)$/);
			if (!blockMatch) return;
			
			const blockId = blockMatch[1];
			const fullText = linkWrapper.textContent || '';
			const urlMatch = fullText.match(/\]\(\/notes\/([^#]+)#([a-fA-F0-9]{6})\)$/);
			if (!urlMatch) return;
			
			const targetNoteId = urlMatch[1];
			linkWrapper.classList.add('transclusion-wrapper');
			
			// Load block content for the tooltip and CSS rendering — no DOM injection
			const cacheKey = `${targetNoteId}#${blockId}`;
			if (blockCache[cacheKey]) {
				const plainText = blockCache[cacheKey].replace(/<[^>]+>/g, '');
				(linkWrapper as HTMLElement).title = plainText;
				(linkWrapper as HTMLElement).setAttribute('data-block-content', plainText);
			} else {
				invoke<NoteDocument>('load_note', { noteId: targetNoteId }).then(n => {
					const blocks = parseBlocks(n.body);
					const targetBlock = blocks.find(b => b.id === blockId);
					if (targetBlock) {
						const rawMd = targetBlock.original.replace(/\s*\(\([a-fA-F0-9]+\)\)$/, '').trim();
						let htmlText = rawMd;
						htmlText = htmlText.replace(/\[([^\]]+)\]\(([^)]+)\)/g, '<span class="mock-link">$1</span>');
						htmlText = htmlText.replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>');
						htmlText = htmlText.replace(/\*([^*]+)\*/g, '<em>$1</em>');
						blockCache[cacheKey] = htmlText;
						// Set plain-text tooltip and data attribute
						const plainText = htmlText.replace(/<[^>]+>/g, '');
						(linkWrapper as HTMLElement).title = plainText;
						(linkWrapper as HTMLElement).setAttribute('data-block-content', plainText);
					}
				}).catch(() => {});
			}
		});
	}

	function setupTransclusionObserver() {
		if (!vditorContainer) return;
		if (transclusionObserver) transclusionObserver.disconnect();
		
		transclusionObserver = new MutationObserver(() => {
			scanForTransclusions();
		});
		
		transclusionObserver.observe(vditorContainer, { childList: true, subtree: true, characterData: true });
	}

	async function fetchRelatedNotes() {
		if (!draftTags.trim()) {
			relatedNotes = [];
			return;
		}
		try {
			const query = draftTags.split(',')[0].trim();
			if (query) {
				const res = await invoke<SearchResponse>('search_notes', { query });
				relatedNotes = res.results
					.map(r => r.note)
					.filter(n => n.id !== note?.id)
					.slice(0, 5);
			}
		} catch (e) {
			console.error(e);
		}
	}

	function handleAnnotationsChange(anns: PdfAnnotation[]) {
		if (note) {
			note.annotations = anns;
			triggerAutoSave();
		}
	}

	function handleImageExtract(base64: string) {
		appendToNoteBody(`\n\n![Extracted Image](${base64})\n\n`);
	}

	async function saveNote() {
		if (!note) return;
		isBusy = true;
		saveStatus = 'saving';
		try {
			let targetId = note.id;
			if (isMainNotePdf) {
				if (!scratchpadSavedId) {
					const newNote = await invoke<NoteDocument>('create_note', { 
						title: draftTitle,
						sourcePdf: activePdfId
					});
					scratchpadSavedId = newNote.id;
				}
				targetId = scratchpadSavedId;
			}

			const sentTitle = draftTitle;
			const saved = await invoke<NoteDocument>('save_note', {
				noteId: targetId,
				title: sentTitle,
				tags: draftTags
					.split(',')
					.map((tag) => tag.trim())
					.filter(Boolean),
				body: draftBody,
				sourcePdf: activePdfId,
				annotations: note.annotations
			});

			if (!isMainNotePdf) {
				note = saved;
			}
			
			if (draftTitle === sentTitle) {
				draftTitle = saved.title;
			}
			
			saveStatus = 'saved';
			void fetchRelatedNotes();
			if (activeSidebarTab === 'versions') {
				void fetchNoteHistory();
			}
		} catch (err) {
			console.error("Save error:", err);
			saveStatus = 'unsaved';
			message = `Save failed: ${err}`;
		} finally {
			isBusy = false;
		}
	}

	async function deleteNote() {
		if (!note) return;
		isBusy = true;
		try {
			await invoke('delete_note', { noteId: note.id });
			await goto(resolve('/'));
		} finally {
			isBusy = false;
		}
	}

	async function duplicateNote() {
		if (!note) return;
		isBusy = true;
		try {
			const duplicated = await invoke<NoteDocument>('duplicate_note', { noteId: note.id });
			// Navigate and reload
			safeNavigate(`/notes/${encodeURIComponent(duplicated.id)}`);
		} finally {
			isBusy = false;
		}
	}

	async function sendChatMessage() {
		if (!note || !chatInput.trim()) return;
		const userText = chatInput.trim();
		chatInput = '';
		if (chatTextareaEl) chatTextareaEl.style.height = 'auto';
		chatMessages = [...chatMessages, { role: 'user', content: userText }];
		chatMessages = [...chatMessages, { role: 'assistant', content: '', isStreaming: true }];
		
		try {
			await invoke('ask_ai_stream', { noteId: note.id, question: userText, requestId: Date.now().toString() });
		} catch (e) {
			console.error("AI Error:", e);
			failStreamingChatMessage();
		}
	}

	function finishStreamingChatMessage() {
		chatMessages = chatMessages.map(m => {
			if (m.isStreaming) return { ...m, isStreaming: false };
			return m;
		});
		if (note) invoke('save_chat_history', { noteId: note.id, chatHistory: chatMessages });
	}

	function failStreamingChatMessage() {
		chatMessages = chatMessages.map(m => {
			if (m.isStreaming) return { ...m, isStreaming: false, error: true, content: 'Failed to generate response.' };
			return m;
		});
		if (note) invoke('save_chat_history', { noteId: note.id, chatHistory: chatMessages });
	}

	async function fetchNoteHistory() {
		if (!note) return;
		isBusy = true;
		try {
			const history = await invoke<GitCommit[]>('get_note_history', { noteId: note.id });
			noteHistory = history
				.filter(c => c.message && c.message.trim() !== '')
				.sort((a, b) => new Date(b.timestamp).getTime() - new Date(a.timestamp).getTime());
		} catch (e) {
			console.error("Failed to fetch history:", e);
		} finally {
			isBusy = false;
		}
	}

	async function previewVersion(commitHash: string) {
		if (!note) return;
		isBusy = true;
		try {
			let rawContent = await invoke<string>('get_note_version', { noteId: note.id, commitHash });
			if (rawContent.match(/^---\r?\n/)) {
				const match = rawContent.match(/^---\r?\n[\s\S]*?\n---\r?\n/);
				if (match) {
					rawContent = rawContent.slice(match[0].length);
				}
			}
			versionPreviewContent = rawContent;
			versionPreviewHash = commitHash;
			if (versionPreviewDialog) {
				versionPreviewDialog.showModal();
			}
		} catch (e) {
			console.error("Failed to fetch version:", e);
		} finally {
			isBusy = false;
		}
	}

	async function restoreVersion(commitHash: string) {
		if (!note) return;
		isBusy = true;
		try {
			let rawContent = await invoke<string>('get_note_version', { noteId: note.id, commitHash });
			if (rawContent.match(/^---\r?\n/)) {
				const match = rawContent.match(/^---\r?\n[\s\S]*?\n---\r?\n/);
				if (match) {
					rawContent = rawContent.slice(match[0].length);
				}
			}
			draftBody = rawContent;
			if (vditorInstance) {
				vditorInstance.setValue(rawContent);
			}
			versionPreviewContent = null;
			versionPreviewHash = null;
			if (versionPreviewDialog) {
				versionPreviewDialog.close();
			}
			triggerAutoSave();
			activeSidebarTab = 'info';
		} catch (e) {
			console.error("Failed to restore version:", e);
		} finally {
			isBusy = false;
		}
	}

	let isProgrammaticNavigation = false;

	function safeNavigate(url: string) {
		if (saveStatus === 'saving' || saveStatus === 'unsaved') {
			pendingNavigationUrl = url;
			navigationWarningDialog?.showModal();
			return;
		}
		isProgrammaticNavigation = true;
		void goto(url);
	}

	function requestDeleteAttachedNote() {
		deleteAttachedNoteDialog?.showModal();
	}

	async function confirmDeleteAttachedNote() {
		deleteAttachedNoteDialog?.close();
		const targetId = isMainNotePdf ? scratchpadSavedId : note?.sourcePdf ? note.id : null;
		const sourcePdfId = isMainNotePdf ? activePdfId : (note?.sourcePdf ?? activePdfId);
		isBusy = true;
		try {
			if (targetId) {
				await invoke('delete_note', { noteId: targetId });
			}
			if (!isMainNotePdf && sourcePdfId) {
				isProgrammaticNavigation = true;
				await goto(`/notes/${encodeURIComponent(sourcePdfId)}`);
				return;
			}
			if (saveTimer) {
				clearTimeout(saveTimer);
				saveTimer = null;
			}
			destroyEditorInstance();
			draftBody = '';
			scratchpadSavedId = null;
			showAttachedNote = false;
			saveStatus = 'saved';
			message = '';
		} finally {
			isBusy = false;
		}
	}

	function cancelDeleteAttachedNote() {
		deleteAttachedNoteDialog?.close();
	}

	function buildPreviewExpandHref() {
		const targetId = previewNoteTarget?.sourcePdf ?? previewNoteTarget?.id;
		const currentNoteId = note?.id;
		if (!targetId) return null;
		const basePath = `/notes/${encodeURIComponent(targetId)}`;
		if (!currentNoteId) return basePath;
		return `${basePath}?returnTo=/notes/${encodeURIComponent(currentNoteId)}`;
	}

	function expandPreviewNoteDirect() {
		const href = buildPreviewExpandHref();
		if (!href) return;
		previewNoteDialog?.close();
		isProgrammaticNavigation = true;
		window.location.href = href;
	}

	function handleBeforeUnload(e: BeforeUnloadEvent) {
		if (isProgrammaticNavigation) return;
		if (saveStatus === 'saving' || saveStatus === 'unsaved') {
			e.preventDefault();
			e.returnValue = '';
		}
	}

	beforeNavigate(({ cancel, to }) => {
		if (isProgrammaticNavigation) return;
		if (saveStatus === 'saving' || saveStatus === 'unsaved') {
			pendingNavigationUrl = to?.url ? `${to.url.pathname}${to.url.search}${to.url.hash}` : '';
			navigationWarningDialog?.showModal();
			cancel();
		}
	});

	function confirmNavigation() {
		navigationWarningDialog?.close();
		if (pendingNavigationUrl) {
			isProgrammaticNavigation = true;
			void goto(pendingNavigationUrl);
			pendingNavigationUrl = '';
		}
	}

	function cancelNavigation() {
		navigationWarningDialog?.close();
		pendingNavigationUrl = '';
	}

	// AI Actions
	async function runExtract() {
		if (!note || !vditorInstance) return;
		isBusy = true;
		try {
			message = 'Extracting from paste...';
			const res = await invoke<string>('extract_from_paste', { noteId: note.id, pasteContent: draftBody });
			const append = `\n\n> AI Extraction:\n${res}`;
			vditorInstance.insertValue(append);
			message = 'Extraction appended.';
		} finally {
			isBusy = false;
		}
	}

	async function runSummarise() {
		if (!note) return;
		isBusy = true;
		try {
			message = 'Summarising note...';
			const res = await invoke<string>('summarise_note', { noteId: note.id });
			alert(`AI Summary:\n\n${res}`);
			message = 'Summary complete.';
		} finally {
			isBusy = false;
		}
	}

	async function runAskAI() {
		if (!note) return;
		const q = prompt('What would you like to ask about this note?');
		if (!q) return;
		isBusy = true;
		try {
			message = 'Asking AI...';
			const res = await invoke<string>('ask_ai', { noteId: note.id, question: q });
			alert(`AI Answer:\n\n${res}`);
			message = 'AI answered.';
		} finally {
			isBusy = false;
		}
	}

	function updateToolbarOverflow() {
		const toolbar = vditorContainer?.querySelector('.vditor-toolbar');
		if (!toolbar) return;
		const items = toolbar.querySelectorAll('.vditor-toolbar__item, .vditor-toolbar__divider');
		items.forEach((item: any) => {
			if (!toolbarExpanded && item.offsetTop > 20) {
				item.style.visibility = 'hidden';
				item.style.pointerEvents = 'none';
			} else {
				item.style.visibility = 'visible';
				item.style.pointerEvents = 'auto';
			}
		});
	}

	$effect(() => {
		const _trigger = toolbarExpanded;
		updateToolbarOverflow();
	});

	function handleGlobalSelectionChange() {
		if (!vditorContainer) return;
		const sel = window.getSelection();
		
		// Clean up previous expansion
		vditorContainer.querySelectorAll('.force-expand').forEach(el => {
			el.classList.remove('force-expand');
		});

		// Only expand if there's an active text selection (not collapsed)
		if (!sel || sel.rangeCount === 0 || sel.isCollapsed) return;

		// Expand links that intersect the current selection
		const links = vditorContainer.querySelectorAll('[data-type="a"]');
		links.forEach(link => {
			if (sel.containsNode(link, true)) {
				link.classList.add('force-expand');
			}
		});
	}

	onMount(() => {
		let unlistenChunk: UnlistenFn;
		let unlistenDone: UnlistenFn;
		let unlistenError: UnlistenFn;

		$showSidebarToggle = true;
		if (window.innerWidth > 1200) {
			$sidebarOpen = true;
		}

		const mql = window.matchMedia('(max-width: 1200px)');
		const handleMediaChange = (e: MediaQueryListEvent) => {
			if (e.matches) {
				$sidebarOpen = false;
			} else {
				$sidebarOpen = true;
			}
		};
		mql.addEventListener('change', handleMediaChange);
		document.addEventListener('selectionchange', handleGlobalSelectionChange);

		let unlistenTool: () => void;

		// Setup AI Streaming listeners
		listen<{ tool: string, details: string }>('ai://chat_tool', (event) => {
			chatMessages = chatMessages.map(m => {
				if (m.isStreaming) {
					const tools = m.tools || [];
					return { ...m, tools: [...tools, { name: event.payload.tool, details: event.payload.details }] };
				}
				return m;
			});
		}).then(fn => unlistenTool = fn);

		listen<{ delta: string, requestId: string }>('ai://chat_chunk', (event) => {
			chatMessages = chatMessages.map(m => {
				if (m.isStreaming) return { ...m, content: m.content + event.payload.delta };
				return m;
			});
		}).then(fn => unlistenChunk = fn);

		listen('ai://chat_done', () => {
			finishStreamingChatMessage();
		}).then(fn => unlistenDone = fn);

		listen('ai://chat_error', () => {
			failStreamingChatMessage();
		}).then(fn => unlistenError = fn);

		window.addEventListener('mousemove', handleGlobalMouseMove);
		window.addEventListener('mouseup', stopResizing);
		window.addEventListener('beforeunload', handleBeforeUnload);

		return () => {
			mql.removeEventListener('change', handleMediaChange);
			document.removeEventListener('selectionchange', handleGlobalSelectionChange);
			window.removeEventListener('mousemove', handleGlobalMouseMove);
			window.removeEventListener('mouseup', stopResizing);
			window.removeEventListener('beforeunload', handleBeforeUnload);
			$showSidebarToggle = false;

			if (unlistenChunk) unlistenChunk();
			if (unlistenDone) unlistenDone();
			if (unlistenError) unlistenError();
			if (unlistenTool) unlistenTool();
		};
	});

	onDestroy(() => {
		if (toolbarResizeObserver) toolbarResizeObserver.disconnect();
		if (vditorInstance) vditorInstance.destroy();
		if (typeof document !== 'undefined') {
			document.removeEventListener('selectionchange', handleGlobalSelectionChange);
		}
	});

	$effect(() => {
		const routeNoteId = page.params.id;
		if (!routeNoteId || routeNoteId === loadedRouteNoteId) return;
		loadedRouteNoteId = routeNoteId;
		void loadCurrentNote(routeNoteId);
	});
</script>

<svelte:head>
	<title>{note ? note.title : 'Myelin'}</title>
	<style>
		/* Bruteforce hide the left scrollbar in split view to prevent Svelte scoping issues */
		.vditor-sv::-webkit-scrollbar {
			display: none !important;
			width: 0 !important;
			background: transparent !important;
		}
		.vditor-sv {
			scrollbar-width: none !important;
			-ms-overflow-style: none !important;
		}
	</style>
</svelte:head>

<div class="editor-shell">
	<header class="editor-header">
		<div class="header-copy">
			<button class="back-link" onclick={() => safeNavigate(backUrl)} aria-label="Go back" title="Go back">
				<svg viewBox="0 0 24 24" width="20" height="20" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
					<line x1="19" y1="12" x2="5" y2="12"></line>
					<polyline points="12 19 5 12 12 5"></polyline>
				</svg>
			</button>
			{#if message}
				<p class="status">{message}</p>
			{/if}
			<input class="title-input" bind:value={draftTitle} oninput={triggerAutoSave} placeholder="Note title" />
			<div class="save-indicator" class:saving={saveStatus === 'saving'}>
				{#if saveStatus === 'saving'}
					<svg class="spinner" viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="10"></circle><path d="M12 6v6l4 2"></path></svg> Saving
				{:else if saveStatus === 'unsaved'}
					<span class="dot"></span> Unsaved
				{:else}
					<svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"></polyline></svg> Saved
				{/if}
			</div>
		</div>
	</header>

	<div class="main-layout" class:split-layout={activePdfBytes !== null && showAttachedNote} bind:this={mainLayoutEl}>
		{#if activePdfBytes}
			<section class="pdf-pane" style="width: {!showAttachedNote ? '100%' : `${splitRatio}%`}">
				<PdfViewer 
					pdfBytes={activePdfBytes} 
					annotations={note?.annotations || []}
					onQuote={handlePdfQuote} 
					onAnnotationsChange={handleAnnotationsChange}
					onImageExtract={handleImageExtract}
					onAttachNote={() => {
						showAttachedNote = true;
						setTimeout(() => initVditor(), 100);
					}}
					showAttachButton={!showAttachedNote}
				/>
			</section>
			{#if showAttachedNote}
				<!-- svelte-ignore a11y_no_static_element_interactions -->
				<div class="resizer" onmousedown={startResizing} class:resizing={isResizing}></div>
			{/if}
		{/if}

		<!-- Main Content Area -->
		{#if shouldRenderEditor}
		<section class="main-pane" style={activePdfBytes ? `width: ${100 - splitRatio}%` : ''}>
			<div class="content-area" style="position: relative;">
				<!-- svelte-ignore a11y_click_events_have_key_events -->
				<!-- svelte-ignore a11y_no_static_element_interactions -->
				<div bind:this={vditorContainer} class="vditor-wrapper" class:toolbar-expanded={toolbarExpanded} class:has-pdf-note={!!activePdfBytes} onclickcapture={handleVditorClick} onkeydowncapture={handleVditorKeydownCapture} onkeyupcapture={handleVditorKeyupCapture} onwheelcapture={(e) => { if (e.ctrlKey || e.metaKey) { e.preventDefault(); e.stopPropagation(); } }}></div>
				<div class="fullscreen-indicator">
					Press <span>{fullscreenShortcut}</span> to toggle
				</div>
				{#if activePdfBytes}
					<div class="toolbar-close-note-container" style={toolbarNeedsToggle ? 'right: 50px;' : 'right: 12px;'}>
						<button
							class="toolbar-close-note-btn"
							onclick={requestDeleteAttachedNote}
							disabled={isBusy}
							title="Delete attached note and close pane"
						>
							Close Note
						</button>
					</div>
				{/if}
				{#if toolbarNeedsToggle}
					<div class="toolbar-overlay-toggle-container">
						<button class="toolbar-overlay-toggle" class:expanded={toolbarExpanded} onclick={() => toolbarExpanded = !toolbarExpanded} aria-label="Toggle toolbar">
							<svg viewBox="0 0 24 24" width="16" height="16" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
								<polyline points="6 9 12 15 18 9"></polyline>
							</svg>
						</button>
					</div>
				{/if}
			</div>
		</section>

		<!-- Right Sidebar -->
		{#if $sidebarOpen}
			<!-- svelte-ignore a11y_click_events_have_key_events -->
			<!-- svelte-ignore a11y_no_static_element_interactions -->
			<div class="sidebar-backdrop" onclick={() => $sidebarOpen = false}></div>
		{/if}
		<aside class="sidebar" class:open={$sidebarOpen} style="--sidebar-width: {sidebarWidth}px;">
			<!-- svelte-ignore a11y_no_static_element_interactions -->
			<div class="sidebar-resizer" onmousedown={startSidebarResizing} class:resizing={isSidebarResizing}></div>
			<div class="sidebar-tabs">
				<button class:active={activeSidebarTab === 'info'} onclick={() => activeSidebarTab = 'info'}>Info</button>
				<button class:active={activeSidebarTab === 'chat'} onclick={() => activeSidebarTab = 'chat'}>Chat</button>
				<button class:active={activeSidebarTab === 'versions'} onclick={() => { activeSidebarTab = 'versions'; fetchNoteHistory(); }}>History</button>
			</div>

			<div class="sidebar-content">
				{#if activeSidebarTab === 'info'}
					<div class="sidebar-section">
						<h3>Tags</h3>
						<input class="tag-input" bind:value={draftTags} oninput={triggerAutoSave} placeholder="comma,separated,tags" onblur={fetchRelatedNotes} />
					</div>

					<div class="sidebar-section">
						<h3>AI Actions</h3>
						<div class="ai-actions">
							<button class="secondary" onclick={runExtract} disabled={isBusy || !note}>✨ Extract from paste</button>
							<button class="secondary" onclick={runSummarise} disabled={isBusy || !note}>✨ Summarise</button>
							<button class="secondary" onclick={runAskAI} disabled={isBusy || !note}>✨ Ask AI about this note</button>
						</div>
					</div>

					<div class="sidebar-section">
						<h3>Related Notes</h3>
						{#if relatedNotes.length > 0}
							<ul class="related-list">
								{#each relatedNotes as rel, i (rel.id + '_' + i)}
									<li><a href="/notes/{encodeURIComponent(rel.id)}">{rel.title}</a></li>
								{/each}
							</ul>
						{:else}
							<p class="empty-state">No related notes found.</p>
						{/if}
					</div>

					<div class="sidebar-section">
						<h3>Backlinks</h3>
						{#if note && note.backlinks && note.backlinks.length > 0}
							<ul class="related-list">
								{#each note.backlinks as link, i (link.sourceId + '_' + (link.targetBlock || '') + '_' + i)}
									<li>
										<a href="/notes/{encodeURIComponent(link.sourceId)}">
											<strong>{link.sourceTitle}</strong>
											{#if link.targetBlock}
												<span style="opacity: 0.7; font-size: 0.8em;">#{link.targetBlock}</span>
											{/if}
										</a>
										<p class="context-excerpt" style="font-size: 0.75rem; color: var(--text-secondary); margin-top: 0.25rem; line-height: 1.4;">
											{@html parseBacklinkContext(link.contextExcerpt)}
										</p>
									</li>
								{/each}
							</ul>
						{:else}
							<p class="empty-state">No backlinks yet.</p>
						{/if}
					</div>

				{:else if activeSidebarTab === 'chat'}
					<div class="chat-container">
						<div class="chat-messages">
							{#if chatMessages.length === 0}
								<p class="empty-state">Ask me anything about this note or your library!</p>
							{:else}
								{#each chatMessages as msg, i}
									<div class="chat-message {msg.role}">
										<div class="chat-bubble">
											{#if msg.tools && msg.tools.length > 0}
												<div class="chat-tools">
													{#each msg.tools as tool}
														<div class="chat-tool-indicator">
															<span class="tool-icon">⚡</span> 
															<span class="tool-name">{tool.name}</span>
															{#if tool.details}
																<span class="tool-details">{tool.details.substring(0, 30)}{tool.details.length > 30 ? '...' : ''}</span>
															{/if}
														</div>
													{/each}
												</div>
											{/if}
											{@html msg.content || (msg.isStreaming && (!msg.tools || msg.tools.length === 0) ? '<span class="loading-dots"></span>' : '')}
										</div>
									</div>
								{/each}
							{/if}
						</div>
						<div class="chat-input-area">
							<textarea 
								bind:this={chatTextareaEl}
								bind:value={chatInput}  
								onkeydown={(e) => {
									if (e.key === 'Enter' && !e.shiftKey) {
										e.preventDefault();
										if (chatInput.trim()) sendChatMessage();
									}
								}} 
								oninput={(e) => {
									const target = e.target as HTMLTextAreaElement;
									target.style.height = 'auto';
									target.style.height = `${Math.min(target.scrollHeight, 200)}px`;
								}}
								placeholder="Ask AI..." 
								rows="1"
							></textarea>
						</div>
					</div>

				{:else if activeSidebarTab === 'versions'}
					<div class="versions-container">
						{#if isBusy && noteHistory.length === 0}
							<p class="empty-state">Loading history...</p>
						{:else if noteHistory.length === 0}
							<p class="empty-state">No history found.</p>
						{:else}
							<ul class="history-list">
								{#each noteHistory as commit (commit.hash)}
									<li>
										<div class="commit-header">
											<strong>{commit.message}</strong>
											<span class="commit-date">{new Date(commit.timestamp).toLocaleString()}</span>
										</div>
										<div class="commit-actions">
											<button class="secondary" onclick={() => previewVersion(commit.hash)}>Preview</button>
											<button class="secondary" onclick={() => restoreVersion(commit.hash)}>Restore</button>
										</div>
									</li>
								{/each}
							</ul>
						{/if}
					</div>
				{/if}
			</div>
		</aside>
		{/if}
	</div>
</div>

<dialog bind:this={versionPreviewDialog} class="version-preview-dialog" onclose={() => versionPreviewContent = null}>
	<div class="dialog-content" style="max-width: 800px; width: 90vw;">
		<h3>Version Preview</h3>
		<div class="preview-content" style="max-height: 60vh; overflow-y: auto; background: var(--bg-page); padding: 1rem; border-radius: var(--radius-sm); border: 1px solid var(--border-default); white-space: pre-wrap; font-family: var(--font-mono); font-size: 0.875rem; margin: 1rem 0;">
			{versionPreviewContent || 'Loading...'}
		</div>
		<div class="dialog-actions">
			<button class="secondary" onclick={() => versionPreviewDialog?.close()}>Close</button>
			{#if versionPreviewHash}
				<button class="primary" onclick={() => restoreVersion(versionPreviewHash!)}>Restore This Version</button>
			{/if}
		</div>
	</div>
</dialog>

<dialog bind:this={mathDialog} class="math-dialog" onclose={() => mathValue = ''}>
	<div class="dialog-content">
		<h3>Insert Math</h3>
		<div class="math-container">
			<svelte:element
				this={"math-field"}
				oninput={(e: any) => (mathValue = e.target.value)}
				style="width: 100%; font-size: 1.5rem; padding: 0.5rem; background: var(--bg-panel); color: var(--text-primary); border: 1px solid var(--border-default); border-radius: var(--radius-xs);"
			>{mathValue}</svelte:element>
		</div>
		<div class="dialog-actions">
			<button class="secondary" onclick={() => mathDialog?.close()}>Cancel</button>
			<button class="primary" onclick={insertMath} disabled={!mathValue}>Insert</button>
		</div>
	</div>
</dialog>

<dialog bind:this={linkNoteDialog} class="link-dialog" onkeydown={handleLinkSearchKeydown} onclose={() => { linkSearchQuery = ''; linkSearchResults = []; linkSelectedIndex = 0; linkDialogMode = 'notes'; if (shouldRefocusEditor) refocusEditorSoon(); }}>
	<div class="dialog-content">
		{#if linkDialogMode === 'notes'}
			<h3>Link to Note</h3>
			<p style="font-size: 0.875rem; color: var(--text-secondary); margin-bottom: var(--space-4);">Search and select a note to link your highlighted text to.</p>
			
			<input class="link-search-input" bind:value={linkSearchQuery} oninput={() => linkSelectedIndex = 0} use:autofocus placeholder="Search notes..." />
			
			{#if linkSearchQuery.trim() || linkSearchResults.length > 0}
			<div class="link-results-container">
				{#if linkSearchResults.length > 0}
					<ul class="link-results-list">
						{#each linkSearchResults as res, i (res.id + '_' + i)}
							<li>
								<button class="link-result-btn" class:selected={i === linkSelectedIndex} onclick={() => selectNoteForBlocks(res)}>
									<strong>{res.title}</strong>
									<span class="folder-badge">{res.folder}</span>
								</button>
							</li>
						{/each}
					</ul>
				{:else if linkSearchQuery.trim()}
					<p class="empty-state">No notes found matching your search.</p>
				{/if}
			</div>
			{/if}
		{:else}
			<h3>Select Block to Reference</h3>
			<p style="font-size: 0.875rem; color: var(--text-secondary); margin-bottom: var(--space-4);">Select a specific block from <strong>{selectedNoteForBlocks?.title}</strong> or link the entire note.</p>
			
			<input class="link-search-input" bind:value={linkSearchQuery} oninput={() => linkSelectedIndex = 0} use:autofocus placeholder="Search blocks..." />
			
			<div class="link-results-container">
				{#if filteredBlocks.length > 0}
					<ul class="link-results-list">
						{#each filteredBlocks as block, i}
							<li>
								<button class="link-result-btn" class:selected={i === linkSelectedIndex} onclick={() => insertBlockLink(block)}>
									<span style={block.isFullNote ? 'font-weight: bold;' : 'font-size: 0.9em; opacity: 0.9; display: -webkit-box; -webkit-line-clamp: 2; -webkit-box-orient: vertical; overflow: hidden;'}>
										{block.text}
									</span>
								</button>
							</li>
						{/each}
					</ul>
				{:else}
					<p class="empty-state">No matching blocks found.</p>
				{/if}
			</div>
		{/if}
		
		<div class="dialog-actions">
			{#if linkDialogMode === 'blocks'}
				<button class="secondary" style="margin-right: auto;" onclick={() => { linkDialogMode = 'notes'; linkSearchQuery = ''; linkSelectedIndex = 0; }}>Back</button>
			{/if}
			<button class="secondary" onclick={() => linkNoteDialog?.close()}>Cancel</button>
		</div>
	</div>
</dialog>

<dialog bind:this={globalSearchDialog} class="link-dialog" onkeydown={handleGlobalSearchKeydown} onclose={() => { globalSearchQuery = ''; globalSelectedIndex = 0; globalBlocks = []; if (shouldRefocusEditor) refocusEditorSoon(); }}>
	<div class="dialog-content">
		<h3>Search Global Blocks</h3>
		<p style="font-size: 0.875rem; color: var(--text-secondary); margin-bottom: var(--space-4);">Search blocks across all notes.</p>
		
		<input class="link-search-input" bind:value={globalSearchQuery} oninput={() => globalSelectedIndex = 0} placeholder="Search global blocks..." />
		
		<div class="link-results-container">
			{#if filteredGlobalBlocks.length > 0}
				<ul class="link-results-list">
					{#each filteredGlobalBlocks as block, i}
						<li>
							<button class="link-result-btn" class:selected={i === globalSelectedIndex} onclick={() => insertGlobalBlockLink(block)}>
								<div>
									<span style="font-size: 0.9em; opacity: 0.9; display: -webkit-box; -webkit-line-clamp: 2; -webkit-box-orient: vertical; overflow: hidden; text-align: left;">
										{block.text}
									</span>
									<span style="font-size: 0.7em; opacity: 0.6; display: block; margin-top: 2px; text-align: left;">
										From: {block.sourceNoteTitle}
									</span>
								</div>
							</button>
						</li>
					{/each}
				</ul>
			{:else}
				<p class="empty-state">{globalBlocks.length > 0 ? "No matching blocks found." : "Loading blocks..."}</p>
			{/if}
		</div>
		
		<div class="dialog-actions">
			<button class="secondary" onclick={() => globalSearchDialog?.close()}>Cancel</button>
		</div>
	</div>
</dialog>

<dialog bind:this={previewNoteDialog} class="preview-dialog" onclose={() => { previewNoteTarget = null; }}>
	{#if previewNoteTarget}
		<div class="preview-layout">
			<div class="preview-main">
				<div class="preview-header">
					<h2>{previewNoteTarget.title}</h2>
					<div class="preview-meta">
						{#if previewNoteTarget.tags.length > 0}
							<span>{previewNoteTarget.tags.join(', ')}</span>
						{/if}
					</div>
				</div>
				<div class="preview-content-scroll">
					<div bind:this={previewNoteContainer} class="vditor-reset" style="padding: 2rem; min-height: 100%;"></div>
				</div>
			</div>
			<div class="preview-sidebar">
				<button class="icon-btn" onclick={() => previewNoteDialog?.close()} title="Close Preview">
					<svg viewBox="0 0 24 24" width="24" height="24" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round"><line x1="18" y1="6" x2="6" y2="18"></line><line x1="6" y1="6" x2="18" y2="18"></line></svg>
				</button>
				<button class="icon-btn" onclick={expandPreviewNoteDirect} title="Expand Note">
					<svg viewBox="0 0 24 24" width="24" height="24" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round"><path d="M15 3h6v6"></path><path d="M9 21H3v-6"></path><path d="M21 3l-7 7"></path><path d="M3 21l7-7"></path></svg>
				</button>
			</div>
		</div>
	{/if}
</dialog>

<dialog bind:this={navigationWarningDialog} class="dialog math-dialog" onclose={cancelNavigation}>
	<div class="dialog-content">
		<h3 style="margin-top: 0;">Unsaved Changes</h3>
		<p style="color: var(--text-secondary); margin-bottom: var(--space-6);">
			The document is currently saving. Are you sure you want to leave? Unsaved changes may be lost.
		</p>
		<div class="dialog-actions">
			<button class="secondary" onclick={cancelNavigation}>Cancel</button>
			<button class="primary" onclick={confirmNavigation}>Leave Page</button>
		</div>
	</div>
</dialog>

<dialog bind:this={deleteAttachedNoteDialog} class="dialog math-dialog" onclose={cancelDeleteAttachedNote}>
	<div class="dialog-content">
		<h3 style="margin-top: 0;">Delete Attached Note</h3>
		<p style="color: var(--text-secondary); margin-bottom: var(--space-6);">
			All data in this attached note will be deleted permanently, and the note pane will be closed.
		</p>
		<div class="dialog-actions">
			<button class="secondary" onclick={cancelDeleteAttachedNote}>Cancel</button>
			<button class="danger" onclick={confirmDeleteAttachedNote} disabled={isBusy}>Delete Note</button>
		</div>
	</div>
</dialog>

<style>
	.editor-shell {
		height: 100vh;
		display: grid;
		grid-template-rows: auto 1fr;
		animation: fade-in var(--duration-page) var(--ease-out);
		background: var(--bg-page);
	}

	.editor-header {
		display: flex;
		justify-content: space-between;
		align-items: center;
		padding: var(--space-4) var(--space-6) var(--space-4) var(--space-8);
		border-bottom: 1px solid var(--border-default);
		background: rgba(16, 16, 16, 0.94);
		backdrop-filter: blur(var(--blur-md));
		position: relative;
		z-index: 1;
	}

	.header-copy {
		display: flex;
		align-items: center;
		gap: var(--space-4);
		flex: 1;
	}

	.back-link,
	input {
		font: inherit;
		font-family: var(--font-mono);
	}

	.back-link {
		border: 1px solid var(--border-subtle);
		border-radius: var(--radius-sm);
		background: transparent;
		color: var(--text-secondary);
		padding: 0.5rem;
		display: flex;
		align-items: center;
		justify-content: center;
		cursor: pointer;
		transition: all var(--duration-fast);
	}
	.back-link:hover {
		color: var(--text-primary);
		border-color: var(--neutral-600);
	}

	.status {
		margin: 0;
		font-size: 0.75rem;
		text-transform: uppercase;
		letter-spacing: 0.08em;
		color: var(--text-secondary);
		width: max-content;
	}

	.title-input {
		font-size: 1.5rem;
		font-weight: 600;
		color: var(--text-hero);
		background: transparent;
		border: 1px solid transparent;
		padding: 0.25rem 0.5rem;
		font-family: var(--font-sans);
		flex: 1;
		max-width: 30rem;
	}
	.title-input:hover, .title-input:focus {
		border-color: var(--border-subtle);
		background: var(--bg-panel);
	}

	.save-indicator {
		display: flex;
		align-items: center;
		gap: 0.375rem;
		font-size: 0.75rem;
		color: var(--text-secondary);
		font-family: var(--font-mono);
		text-transform: uppercase;
		letter-spacing: 0.05em;
		padding: 0.25rem 0.5rem;
		margin-left: auto;
	}

	.save-indicator.saving {
		color: var(--accent-100);
	}

	.save-indicator .dot {
		width: 6px;
		height: 6px;
		border-radius: 50%;
		background: var(--neutral-400);
	}

	.spinner {
		animation: spin 1s linear infinite;
	}

	@keyframes spin {
		100% { transform: rotate(360deg); }
	}
	button:disabled { opacity: 0.6; cursor: not-allowed; }

	.main-layout {
		min-height: 0;
		position: relative;
		display: flex;
		overflow: hidden;
		z-index: 20; /* Ensures tooltips render above the header's stacking context */
	}

	.pdf-pane,
	.main-pane {
		min-width: 18rem;
	}

	.resizer {
		width: 10px;
		flex: 0 0 10px;
		cursor: col-resize;
		position: relative;
		background:
			linear-gradient(
				90deg,
				transparent 0,
				transparent 3px,
				var(--border-subtle) 3px,
				var(--border-subtle) 7px,
				transparent 7px
			);
		transition: background 0.2s ease;
	}

	.resizer:hover,
	.resizer.resizing {
		background:
			linear-gradient(
				90deg,
				transparent 0,
				transparent 2px,
				var(--accent-100) 2px,
				var(--accent-100) 8px,
				transparent 8px
			);
	}

	.sidebar-backdrop {
		display: none;
	}

	/* Main Pane */
	.main-pane {
		flex: 1;
		display: flex;
		flex-direction: column;
		background: var(--bg-page);
		align-items: stretch;
		min-height: 0;
	}

	.danger {
		border: 1px solid rgba(239, 68, 68, 0.35);
		background: rgba(239, 68, 68, 0.12);
		color: #fecaca;
	}

	.danger:hover:not(:disabled) {
		background: rgba(239, 68, 68, 0.18);
		color: #fee2e2;
	}

	.content-area {
		width: 100%;
		flex: 1;
		display: flex;
		flex-direction: column;
		min-height: 0;
	}

	.vditor-wrapper {
		flex: 1;
		min-height: 0;
		border: none !important;
	}

	:global(.vditor-reset) {
		padding-top: var(--space-6) !important;
	}

	.toolbar-overlay-toggle-container {
		position: absolute;
		top: 0;
		right: 0;
		height: 48px;
		padding-right: var(--space-4);
		display: flex;
		align-items: center;
		justify-content: center;
		z-index: 100;
	}

	.toolbar-close-note-container {
		position: absolute;
		top: 0;
		height: 48px;
		display: flex;
		align-items: center;
		justify-content: center;
		z-index: 101;
		pointer-events: none;
	}

	.toolbar-close-note-btn {
		pointer-events: auto;
		border: 1px solid rgba(239, 68, 68, 0.35);
		background: rgba(239, 68, 68, 0.12);
		color: #fecaca;
		border-radius: var(--radius-sm);
		padding: 0.4rem 0.75rem;
		height: 32px;
		font-size: 0.82rem;
		font-family: var(--font-mono);
		line-height: 1;
		white-space: nowrap;
	}

	.toolbar-close-note-btn:hover:not(:disabled) {
		background: rgba(239, 68, 68, 0.18);
		color: #fee2e2;
	}

	.toolbar-overlay-toggle {
		width: 26px;
		height: 26px;
		display: flex;
		align-items: center;
		justify-content: center;
		background: transparent;
		border: none;
		border-radius: var(--radius-xs);
		color: var(--text-secondary);
		cursor: pointer;
		transition: all 0.2s;
	}
	.toolbar-overlay-toggle:hover {
		color: var(--text-primary);
		background: rgba(255, 255, 255, 0.05);
	}
	.toolbar-overlay-toggle.expanded svg {
		transform: rotate(180deg);
	}

	:global(.vditor-wrapper:not(.toolbar-expanded) .vditor-toolbar) {
		max-height: 48px;
		overflow: visible !important;
	}

	/* Force all upward-facing tooltips (__n, __ne, __nw) to point downwards vertically */
	:global(.vditor-toolbar .vditor-tooltipped__n::after),
	:global(.vditor-toolbar .vditor-tooltipped__ne::after),
	:global(.vditor-toolbar .vditor-tooltipped__nw::after) {
		bottom: auto !important;
		top: 100% !important;
		margin-bottom: 0 !important;
		margin-top: 5px !important;
	}

	:global(.vditor-toolbar .vditor-tooltipped__n::before),
	:global(.vditor-toolbar .vditor-tooltipped__ne::before),
	:global(.vditor-toolbar .vditor-tooltipped__nw::before) {
		top: auto !important;
		bottom: -5px !important;
		border-top-color: transparent !important;
		border-bottom-color: #3b3e43 !important;
	}

	:global(.vditor) {
		border: none !important;
		overflow: visible !important;
		height: 100% !important;
		display: flex !important;
		flex-direction: column !important;
		--panel-background-color: var(--bg-page) !important;
		--textarea-background-color: var(--bg-page) !important;
		--toolbar-background-color: rgba(18, 18, 18, 0.96) !important;
	}

	:global(.vditor-content) {
		display: flex !important;
		flex-direction: column !important;
		align-items: stretch !important;
		width: 100% !important;
		background: var(--bg-page) !important;
		flex: 1 !important;
		min-height: 0 !important;
		overflow: hidden !important;
	}

	:global(.vditor-ir),
	:global(.vditor-sv),
	:global(.vditor-preview) {
		width: 100% !important;
		box-sizing: border-box !important;
		flex: 1 !important;
		min-height: 0 !important;
		overflow-y: auto !important;
	}

	/* Hide the middle scrollbar in Split View (left pane) */
	:global(.vditor-sv)::-webkit-scrollbar {
		display: none !important;
		width: 0 !important;
		background: transparent !important;
	}
	:global(.vditor-sv) {
		scrollbar-width: none !important;
		-ms-overflow-style: none !important;
	}

	:global(.vditor-reset) {
		/* Centering content while keeping scrollbar at the edge */
		padding-left: max(var(--space-6), calc(50% - 105mm)) !important;
		padding-right: max(var(--space-6), calc(50% - 105mm)) !important;
	}

	:global(.vditor-preview__action) {
		display: none !important;
	}

	@media (min-width: 1200px) {
		:global(.vditor-content:has(.vditor-sv[style*="block"])) {
			flex-direction: row !important;
			align-items: stretch !important;
			justify-content: center !important;
			gap: 0 !important;
			padding: 0 !important;
		}
		
		:global(.vditor-content:has(.vditor-sv[style*="block"]) .vditor-ir),
		:global(.vditor-content:has(.vditor-sv[style*="block"]) .vditor-sv),
		:global(.vditor-content:has(.vditor-sv[style*="block"]) .vditor-preview) {
			margin: 0 !important;
		}

		:global(.vditor-content:has(.vditor-sv[style*="block"]) .vditor-reset) {
			padding-left: var(--space-6) !important;
			padding-right: var(--space-6) !important;
		}
	}

	:global(.vditor-reset),
	:global(.vditor-textarea) {
		font-family: var(--font-mono) !important;
	}
	
	:global(.vditor-ir),
	:global(.vditor-reset) {
		color: var(--text-primary) !important;
	}
	
	:global(.vditor-toolbar) {
		border-bottom: 1px solid var(--border-subtle) !important;
		padding: var(--space-2) var(--space-4) !important;
		padding-right: 48px !important;
		transition: max-height 0.2s ease-out;
		position: relative !important;
		z-index: 30 !important;
	}

	:global(.vditor-wrapper.has-pdf-note .vditor-toolbar) {
		padding-right: 160px !important;
	}



	/* Sidebar (Mobile / Overlay mode by default) */
	.sidebar {
		position: absolute;
		top: 0;
		right: 0;
		bottom: 0;
		width: var(--sidebar-width, 20rem);
		max-width: 85vw;
		background: var(--bg-panel);
		padding: 0 var(--space-6) var(--space-6) var(--space-6);
		display: flex;
		flex-direction: column;
		gap: var(--space-6);
		overflow-y: auto;
		z-index: 100;
		transform: translateX(100%);
		transition: transform 0.3s cubic-bezier(0.16, 1, 0.3, 1), margin-right 0.3s cubic-bezier(0.16, 1, 0.3, 1);
		border-left: 1px solid var(--border-default);
		border-radius: 0 !important;
		box-shadow: -4px 0 24px rgba(0, 0, 0, 0.4);
	}

	.sidebar.open {
		transform: translateX(0);
	}

	.sidebar-backdrop {
		position: absolute;
		inset: 0;
		background: rgba(0, 0, 0, 0.4);
		backdrop-filter: blur(var(--blur-sm));
		z-index: 90;
		animation: fade-in var(--duration-fast) ease-out;
	}

	/* Large Screen Styles (Side-by-side docked mode) */
	@media (min-width: 1201px) {
		.sidebar {
			position: relative;
			transform: none;
			margin-right: calc(var(--sidebar-width, 20rem) * -1);
			box-shadow: none;
			flex-shrink: 0;
		}

		.sidebar.open {
			transform: none;
			margin-right: 0;
		}

		.sidebar-backdrop {
			display: none !important;
		}
	}

	.sidebar-resizer {
		position: absolute;
		left: -3px;
		top: 0;
		bottom: 0;
		width: 6px;
		cursor: ew-resize;
		z-index: 1000;
		transition: background 0.2s ease;
	}
	.sidebar-resizer:hover, .sidebar-resizer.resizing {
		background: var(--accent-100);
	}

	.sidebar-section {
		display: flex;
		flex-direction: column;
		gap: var(--space-3);
	}

	.sidebar h3 {
		margin: 0;
		font-size: 0.75rem;
		text-transform: uppercase;
		letter-spacing: 0.08em;
		color: var(--text-secondary);
	}

	.tag-input {
		width: 100%;
		border: 1px solid var(--border-default);
		border-radius: var(--radius-xs);
		background: var(--bg-page);
		padding: 0.625rem 0.75rem;
		color: var(--text-primary);
		outline: none;
	}
	.tag-input:focus { border-color: var(--accent-200); }

	.ai-actions {
		display: flex;
		flex-direction: column;
		gap: var(--space-2);
	}
	
	.ai-actions button {
		text-align: left;
		background: var(--bg-page);
		border: 1px solid var(--border-default);
		color: var(--text-primary);
		padding: 0.625rem 0.75rem;
		border-radius: var(--radius-xs);
		cursor: pointer;
		font-size: 0.875rem;
		font-family: var(--font-sans);
		transition: border-color var(--duration-fast);
	}
	.ai-actions button:hover:not(:disabled) {
		border-color: var(--accent-200);
		color: var(--accent-100);
	}

	.empty-state {
		margin: 0;
		font-size: 0.875rem;
		color: var(--neutral-500);
		font-style: italic;
	}

	.related-list {
		list-style: none;
		padding: 0;
		margin: 0;
		display: flex;
		flex-direction: column;
		gap: var(--space-2);
	}

	.related-list a {
		color: var(--text-primary);
		text-decoration: none;
		font-size: 0.875rem;
		display: block;
		padding: 0.375rem 0;
		border-bottom: 1px solid transparent;
		transition: color var(--duration-fast);
	}
	.related-list a:hover {
		color: var(--accent-100);
	}

	@keyframes fade-in {
		from { opacity: 0; transform: translateY(8px); }
		to { opacity: 1; transform: translateY(0); }
	}

	@media (max-width: 1024px) {
		.editor-header { 
			flex-wrap: wrap; 
			gap: var(--space-4); 
			position: sticky;
			top: 0;
			z-index: 10;
		}
		.title-input { max-width: 100%; }
	}

	.math-dialog {
		padding: 0;
		border: 1px solid var(--border-default);
		border-radius: var(--radius-sm);
		background: var(--bg-panel);
		color: var(--text-primary);
		max-width: 40rem;
		width: 100%;
		backdrop-filter: blur(var(--blur-md));
		box-shadow: none;
	}
	.math-dialog::backdrop {
		background: rgba(0, 0, 0, 0.6);
		backdrop-filter: blur(var(--blur-sm));
	}
	.dialog-content {
		padding: var(--space-6);
		display: grid;
		gap: var(--space-4);
	}
	.dialog-content h3 {
		margin: 0;
		font-size: 1.25rem;
		color: var(--text-hero);
	}
	.math-container {
		min-height: 4rem;
	}
	.dialog-actions {
		display: flex;
		justify-content: flex-end;
		gap: var(--space-2);
		margin-top: var(--space-4);
	}
	.dialog-actions button {
		padding: 0.625rem 1rem;
		border-radius: var(--radius-sm);
		border: 1px solid var(--border-default);
		background: var(--bg-panel);
		color: var(--text-primary);
		cursor: pointer;
	}
	.dialog-actions .primary {
		background: var(--accent-200);
		color: var(--text-inverse);
		border-color: var(--accent-200);
	}
	
	.link-dialog {
		padding: 0;
		border: 1px solid var(--border-default);
		border-radius: var(--radius-sm);
		background: var(--bg-panel);
		color: var(--text-primary);
		max-width: 40rem;
		width: 100%;
		backdrop-filter: blur(var(--blur-md));
		outline: none;
		box-shadow: none;
	}
	.link-dialog::backdrop {
		background: rgba(0, 0, 0, 0.6);
		backdrop-filter: blur(var(--blur-sm));
	}
	.link-search-input {
		width: 100%;
		border: 2px solid var(--border-default);
		border-radius: var(--radius-sm);
		background: var(--bg-panel);
		padding: 1rem 1.25rem;
		font-size: 1.125rem;
		color: var(--text-primary);
		outline: none;
		font-family: var(--font-sans);
		margin-bottom: var(--space-4);
		transition: border-color 0.2s;
	}
	.link-search-input:focus { border-color: var(--accent-200); }
	
	.link-results-container {
		max-height: 300px;
		overflow-y: auto;
		border: 1px solid var(--border-subtle);
		border-radius: var(--radius-xs);
		background: var(--bg-panel);
		padding: var(--space-2);
	}
	
	.link-results-list {
		list-style: none;
		margin: 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: 2px;
	}
	
	.link-result-btn {
		width: 100%;
		display: flex;
		justify-content: space-between;
		align-items: center;
		padding: 0.5rem 0.75rem;
		background: transparent;
		border: none;
		border-radius: var(--radius-xs);
		color: var(--text-primary);
		cursor: pointer;
		text-align: left;
		transition: background 0.1s;
	}
	.link-result-btn:hover,
	.link-result-btn.selected {
		background: rgba(238, 96, 24, 0.12);
	}
	
	.folder-badge {
		font-size: 0.7rem;
		color: var(--text-secondary);
		background: var(--bg-page);
		padding: 0.125rem 0.375rem;
		border-radius: 1rem;
		border: 1px solid var(--border-subtle);
	}

	.preview-dialog {
		padding: 0;
		border: none;
		border-radius: var(--radius-md);
		background: transparent;
		color: var(--text-primary);
		width: 800px;
		max-width: 90vw;
		height: 75vh;
		max-height: 80vh;
		outline: none;
	}
	.preview-dialog::backdrop {
		background: rgba(0, 0, 0, 0.4);
		backdrop-filter: blur(var(--blur-sm));
	}
	.preview-layout {
		display: flex;
		height: 100%;
		gap: var(--space-4);
		position: relative;
	}
	.preview-main {
		flex: 1;
		background: var(--bg-page);
		border-radius: var(--radius-md);
		border: 1px solid var(--border-default);
		display: flex;
		flex-direction: column;
		overflow: hidden;
		box-shadow: 0 12px 48px rgba(0,0,0,0.5);
	}
	.preview-header {
		padding: var(--space-6) var(--space-8);
		border-bottom: 1px solid var(--border-subtle);
		background: var(--bg-panel);
	}
	.preview-header h2 {
		margin: 0 0 var(--space-2) 0;
		font-size: 1.5rem;
		color: var(--text-hero);
	}
	.preview-meta span {
		font-family: var(--font-mono);
		font-size: 0.875rem;
		color: var(--text-secondary);
		background: var(--neutral-600);
		padding: 0.1rem 0.4rem;
		border-radius: var(--radius-xs);
	}
	.preview-content-scroll {
		flex: 1;
		overflow-y: auto;
		background: var(--bg-page);
	}
	.preview-sidebar {
		display: flex;
		flex-direction: column;
		gap: var(--space-3);
		padding-top: var(--space-4);
		align-items: center;
	}
	.icon-btn {
		width: 48px;
		height: 48px;
		border-radius: 50%;
		background: var(--bg-panel);
		border: 1px solid var(--border-default);
		color: var(--text-primary);
		display: flex;
		align-items: center;
		justify-content: center;
		cursor: pointer;
		transition: all 0.2s;
	}
	.icon-btn:hover {
		background: var(--neutral-600);
		color: var(--text-inverse);
	}
	
	/* Base State (Collapsed): Hide the link text completely to make the transclusion look seamless */
	:global(.transclusion-wrapper[data-block-content]:not([data-block-content=""]):not(.vditor-ir__node--expand):not(.force-expand) .vditor-ir__link) {
		display: none !important;
	}
	/* Strip the wrapper pill background when collapsed because we only want the ::after to show */
	:global(.transclusion-wrapper[data-block-content]:not([data-block-content=""]):not(.vditor-ir__node--expand):not(.force-expand)) {
		padding: 0 !important;
		background: transparent !important;
		border: none !important;
		display: block !important;
	}
	/* Ensure the ::after preview has no top margin since there's no text above it */
	:global(.transclusion-wrapper[data-block-content]:not([data-block-content=""]):not(.vditor-ir__node--expand):not(.force-expand)::after) {
		margin-top: 0 !important;
	}

	/* Active State (Selected or Edited): Restore the orange pill styling */
	:global(.transclusion-wrapper.force-expand),
	:global(.transclusion-wrapper.vditor-ir__node--expand) {
		padding: 0.25rem 0.5rem !important;
		background: rgba(238, 96, 24, 0.06) !important;
		border-left: 3px solid var(--accent-200) !important;
		border-radius: 0 var(--radius-sm) var(--radius-sm) 0 !important;
		display: inline-block !important;
	}

	/* Style the link text nicely when active */
	:global(.transclusion-wrapper .vditor-ir__link) {
		color: var(--accent-200) !important;
		font-family: var(--font-mono) !important;
		font-size: 0.875em !important;
	}

	/* Render the block content seamlessly via pseudo-element */
	:global(.transclusion-wrapper::after) {
		content: attr(data-block-content);
		display: block;
		margin-top: 0.5rem;
		padding: var(--space-3) 1rem;
		background: rgba(238, 96, 24, 0.05);
		border-left: 3px solid var(--accent-200);
		border-radius: 0 var(--radius-sm) var(--radius-sm) 0;
		color: var(--text-secondary);
		font-family: var(--font-mono);
		white-space: pre-wrap;
		font-size: 0.85em;
		line-height: 1.5;
		cursor: default;
	}

	/* Hide the transclusion content when the link is actively selected or edited to prevent visual clutter */
	:global(.transclusion-wrapper.force-expand::after),
	:global(.transclusion-wrapper.vditor-ir__node--expand::after) {
		display: none !important;
	}

	/* Prevent Vditor from "truncating" (hiding) link markers ONLY when actively selected or edited */
	:global(.vditor-ir__node[data-type="a"].force-expand .vditor-ir__marker),
	:global(.vditor-ir__node[data-type="a"].vditor-ir__node--expand .vditor-ir__marker) {
		display: inline !important;
		opacity: 0.6;
		font-family: var(--font-mono);
	}
	
	/* Vditor link theme override */
	:global(.vditor-reset a),
	:global(.vditor-ir__link) {
		color: var(--accent-200) !important;
		text-decoration-color: var(--accent-200) !important;
	}

	/* Ensure Vditor fullscreen covers the custom titlebar */
	:global(.vditor--fullscreen) {
		z-index: 10000 !important;
	}

	/* Fullscreen Indicator */
	.fullscreen-indicator {
		display: none;
		position: fixed;
		bottom: var(--space-8);
		right: var(--space-8);
		background: rgba(18, 18, 18, 0.85);
		color: var(--text-secondary);
		padding: var(--space-2) var(--space-4);
		border-radius: var(--radius-full);
		font-size: 0.875rem;
		pointer-events: none;
		z-index: 10001; /* Must be above Vditor's 10000 */
		backdrop-filter: blur(var(--blur-md));
		border: 1px solid var(--border-default);
		box-shadow: var(--shadow-lg);
	}
	
	.fullscreen-indicator span {
		background: var(--bg-panel);
		color: var(--text-primary);
		padding: 2px 6px;
		border-radius: var(--radius-xs);
		border: 1px solid var(--border-subtle);
		font-family: var(--font-mono);
		font-size: 0.75rem;
	}

	:global(.content-area:has(.vditor--fullscreen)) .fullscreen-indicator {
		display: flex;
		align-items: center;
		gap: var(--space-2);
		animation: fade-in 0.3s ease-out;
	}

	:global(.vditor-hint button[data-mode="wysiwyg"]) {
		display: none !important;
	}

	/* Sidebar Tabs */
	.sidebar-tabs {
		display: flex;
		height: 48px;
		border-bottom: 1px solid var(--border-subtle);
		margin-bottom: var(--space-4);
		flex-shrink: 0;
	}
	.sidebar-tabs button {
		flex: 1;
		background: transparent;
		border: none;
		color: var(--text-secondary);
		padding: 0;
		font-family: var(--font-sans);
		font-size: 0.875rem;
		cursor: pointer;
		border-bottom: 2px solid transparent;
		transition: all var(--duration-fast);
		display: flex;
		align-items: center;
		justify-content: center;
	}
	.sidebar-tabs button.active {
		color: var(--accent-100);
		border-bottom-color: var(--accent-100);
	}
	.sidebar-tabs button:hover:not(.active) {
		color: var(--text-primary);
	}

	.sidebar-content {
		display: flex;
		flex-direction: column;
		gap: var(--space-6);
		flex: 1;
		min-height: 0;
		overflow-y: auto;
	}

	.sidebar-content::-webkit-scrollbar,
	.chat-messages::-webkit-scrollbar,
	.versions-container::-webkit-scrollbar,
	.sidebar::-webkit-scrollbar {
		display: none;
	}
	.sidebar-content, .chat-messages, .versions-container, .sidebar {
		-ms-overflow-style: none;
		scrollbar-width: none;
	}

	/* Chat UI */
	.chat-container {
		display: flex;
		flex-direction: column;
		height: 100%;
		flex: 1;
	}
	.chat-messages {
		flex: 1;
		overflow-y: auto;
		display: flex;
		flex-direction: column;
		gap: var(--space-4);
		padding-bottom: var(--space-4);
	}
	.chat-message {
		display: flex;
		flex-direction: column;
	}
	.chat-message.user {
		align-items: flex-end;
	}
	.chat-message.assistant {
		align-items: flex-start;
	}
	.chat-bubble {
		max-width: 85%;
		padding: var(--space-3) var(--space-4);
		border-radius: var(--radius-md);
		font-size: 0.875rem;
		line-height: 1.5;
	}
	.chat-message.user .chat-bubble {
		background: var(--accent-200);
		color: var(--bg-page);
	}
	.chat-message.assistant .chat-bubble {
		background: var(--bg-panel);
		border: 1px solid var(--border-default);
		color: var(--text-primary);
	}
	.chat-tools {
		display: flex;
		flex-direction: column;
		gap: var(--space-2);
		margin-bottom: var(--space-2);
	}
	.chat-tool-indicator {
		display: flex;
		align-items: center;
		gap: var(--space-2);
		font-size: 0.75rem;
		padding: var(--space-1) var(--space-2);
		background: rgba(255, 255, 255, 0.05);
		border-radius: var(--radius-sm);
		border: 1px dashed var(--border-default);
		color: var(--text-secondary);
	}
	.tool-name {
		font-weight: 500;
		color: var(--accent-300);
	}
	.tool-details {
		font-style: italic;
		opacity: 0.8;
	}
	.chat-input-area {
		display: flex;
		gap: var(--space-2);
		margin-top: auto;
		padding-top: var(--space-4);
		border-top: 1px solid var(--border-default);
	}
	.chat-input-area textarea {
		flex: 1;
		background: var(--bg-page);
		border: 1px solid var(--border-default);
		border-radius: var(--radius-xs);
		padding: var(--space-2) var(--space-3);
		color: var(--text-primary);
		outline: none;
		resize: none;
		font-family: inherit;
		line-height: 1.4;
		overflow-y: auto;
	}
	.chat-input-area textarea:focus {
		border-color: var(--accent-200);
	}

	.loading-dots::after {
		content: '...';
		animation: blink 1.5s steps(4, end) infinite;
	}
	@keyframes blink {
		0%, 20% { color: transparent; }
		40% { color: inherit; }
		100% { color: inherit; }
	}

	/* Version History UI */
	.versions-container {
		display: flex;
		flex-direction: column;
		height: 100%;
	}
	.history-list {
		list-style: none;
		padding: 0;
		margin: 0;
		display: flex;
		flex-direction: column;
		gap: var(--space-4);
	}
	.history-list li {
		background: var(--bg-panel);
		border: 1px solid var(--border-default);
		border-radius: var(--radius-sm);
		padding: var(--space-3);
		display: flex;
		flex-direction: column;
		gap: var(--space-2);
	}
	.commit-header {
		display: flex;
		flex-direction: column;
	}
	.commit-header strong {
		font-size: 0.875rem;
		color: var(--text-primary);
	}
	.commit-date {
		font-size: 0.75rem;
		color: var(--text-secondary);
	}
	.commit-actions {
		display: flex;
		gap: var(--space-2);
		margin-top: var(--space-2);
	}
	.commit-actions button {
		flex: 1;
		font-size: 0.75rem;
		padding: var(--space-1) 0;
	}
</style>
