<script lang="ts">
	import { invoke } from '@tauri-apps/api/core';
	import { listen, type UnlistenFn } from '@tauri-apps/api/event';
	import { open as openFileDialog } from '@tauri-apps/plugin-dialog';
	import { goto, beforeNavigate } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import type {
		NoteDocument,
		SearchResponse,
		NoteSummary,
		PdfAnnotation,
		GitCommit,
		ChatMessage
	} from '$lib/types';
	import { onMount, onDestroy, tick } from 'svelte';
	import { noteOpened, noteClosed } from '$lib/llamaWarm';
	import { showSidebarToggle, noteSidebarOpen } from '$lib/stores';
	import Vditor from 'vditor';
	import 'vditor/dist/index.css';
	import 'mathlive';
	import 'mathlive/fonts.css';
	import PdfViewer from '$lib/components/PdfViewer.svelte';
	import EpubViewer from '$lib/components/EpubViewer.svelte';
	import HtmlViewer from '$lib/components/HtmlViewer.svelte';
	import TexEditor from '$lib/components/TexEditor.svelte';
	import IpynbEditor from '$lib/components/IpynbEditor.svelte';
	import ChatToolIndicator from '$lib/components/ChatToolIndicator.svelte';
	import { marked } from 'marked';
	import DOMPurify from 'dompurify';

	let requireToolApproval = $state(false);

	let note = $state<NoteDocument | null>(null);
	let draftBody = $state('');
	let draftTitle = $state('');
	let draftTags = $state('');
	let isBusy = $state(false);
	let message = $state('');

	let activeSidebarTab = $state<'info' | 'chat' | 'versions'>('info');
	let noteHistory = $state<GitCommit[]>([]);
	let versionPreviewContent = $state<string | null>(null);
	let versionPreviewHash = $state<string | null>(null);
	let versionPreviewDialog: HTMLDialogElement | undefined = $state();
	type NoteSnapshot = import('$lib/types').NoteSnapshot;

	let chatMessages = $state<ChatMessage[]>([]);
	let chatInput = $state('');
	// The editor selection the user has "armed" for the AI. Persists across sends
	// (cleared only by the ✕ pill or by deselecting inside the editor). Captured in
	// source-markdown coordinates with surrounding context so the backend can pin
	// the exact span even as the note drifts.
	let armedSelection = $state<{
		text: string;
		before: string;
		after: string;
		chars: number;
		words: number;
	} | null>(null);
	let selDebounce: ReturnType<typeof setTimeout> | undefined;

	// Prompt-box context-usage ring: estimate fill from the open note + chat
	// length against the working context window (~32K tokens ≈ 130K chars).
	const RING_CIRC = 2 * Math.PI * 15.5;
	let contextPercent = $derived.by(() => {
		const noteChars = note?.body?.length ?? 0;
		const histChars = chatMessages.reduce((s, m) => s + (m.content?.length ?? 0), 0);
		const used = noteChars + histChars + chatInput.length;
		return Math.min(100, Math.round((used / 130000) * 100));
	});
	let ringOffset = $derived(RING_CIRC * (1 - contextPercent / 100));
	let ringColor = $derived(
		contextPercent < 60 ? 'var(--accent-200, #6ea8fe)' : contextPercent < 85 ? '#e0a341' : '#e5484d'
	);

	// A chat turn is in flight while the last assistant bubble is still streaming.
	// Sending is blocked until it finishes, but the textarea stays editable so you
	// can compose your next prompt while the model is still answering.
	let isChatStreaming = $derived(chatMessages.some((m) => m.isStreaming));

	// Upload button: attach a document (becomes a note via the PDF/EPUB import).
	async function attachFile() {
		const picked = await openFileDialog({
			multiple: false,
			filters: [{ name: 'Documents', extensions: ['pdf', 'epub'] }]
		});
		if (typeof picked === 'string') {
			try {
				await invoke('import_pdf_file', { filePath: picked });
			} catch (e) {
				console.error('attach failed', e);
			}
		}
	}
	let chatTextareaEl: HTMLTextAreaElement | undefined = $state();
	let chatMessagesEl: HTMLDivElement | undefined = $state();
	let currentTime = $state(Date.now());

	let backUrl = $derived(page.url.searchParams.get('returnTo') || '/');

	let relatedNotes = $state<NoteSummary[]>([]);
	let vditorContainer: HTMLElement | undefined = $state();
	let vditorInstance: Vditor | null = null;
	let fullscreenShortcut = $state('Esc');
	let noteAnimationTimer: ReturnType<typeof setTimeout> | undefined;
	// Live note streaming (real token-by-token writes from the backend).
	let noteStreaming = $state(false);
	let noteStreamBuf = '';
	let noteStreamBackup = '';
	let savedEditorRange: Range | null = null;
	let shouldRefocusEditor = false;

	let isSourceMaterial = $state(false);
	let sourceMaterialType = $state<'pdf' | 'epub' | 'html' | null>(null);
	let workingDocType = $state<'md' | 'tex' | 'ipynb'>('md');
	let activeSourceId = $state<string | null>(null);
	let activeSourceBytes = $state<Uint8Array | null>(null);
	let scratchpadSavedId = $state<string | null>(null);
	let showAttachedNote = $state(false);

	let splitRatio = $state(50);
	let isResizing = $state(false);
	let mainLayoutEl: HTMLElement | undefined = $state();

	const NOTE_MIN_WIDTH = 800; // must match .main-pane min-width in CSS
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
			let newRatio = ((e.clientX - rect.left) / rect.width) * 100;
			if (workingDocType === 'tex') {
				newRatio = 100 - newRatio;
			}
			if (newRatio > 20 && newRatio < 80) {
				splitRatio = newRatio;
			}
		} else if (isSidebarResizing) {
			const newWidth = window.innerWidth - e.clientX;
			// Never let the sidebar grow past the point where the note would drop
			// below its protected minimum width — keeps it on-screen and the editor
			// stable. Clamp against the layout container (which excludes the left
			// rail), not the full window, or the panes overflow and get clipped.
			const containerWidth = mainLayoutEl?.getBoundingClientRect().width ?? window.innerWidth;
			const maxSidebar = Math.max(320, containerWidth - NOTE_MIN_WIDTH);
			sidebarWidth = Math.max(320, Math.min(newWidth, maxSidebar));
		}
	}

	function stopResizing() {
		if (isResizing || isSidebarResizing) {
			isResizing = false;
			if (isSidebarResizing) {
				isSidebarResizing = false;
				localStorage.setItem('myelin_sidebar_width', sidebarWidth.toString());
			}
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

	let userScrolledUp = false;

	function handleChatScroll(e: Event) {
		const el = e.currentTarget as HTMLElement;
		const distanceToBottom = el.scrollHeight - el.scrollTop - el.clientHeight;
		userScrolledUp = distanceToBottom > 50;
	}

	function scrollChatToBottom(force = false) {
		if (!chatMessagesEl) return;
		if (force || !userScrolledUp) {
			chatMessagesEl.scrollTop = chatMessagesEl.scrollHeight;
		}
	}

	$effect(() => {
		if (activeSidebarTab !== 'chat') return;
		const chatScrollKey = chatMessages
			.map(
				(msg) =>
					`${msg.role}:${msg.content.length}:${msg.isStreaming ? 1 : 0}:${msg.tools?.length ?? 0}:${msg.error ? 1 : 0}`
			)
			.join('|');
		void chatScrollKey;
		void tick().then(() => {
			scrollChatToBottom();
		});
	});

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

	// Text offset of a (container, offset) point within the editor's rendered text.
	function textOffsetOf(editorEl: HTMLElement, container: Node, offset: number): number | null {
		const walker = document.createTreeWalker(editorEl, NodeFilter.SHOW_TEXT);
		let acc = 0;
		let node: Node | null;
		while ((node = walker.nextNode())) {
			if (node === container) return acc + offset;
			acc += node.textContent?.length ?? 0;
		}
		return null;
	}

	// Occurrence of `needle` in `hay` whose start is closest to `hint` (disambiguates repeats).
	function nearestIndexOf(hay: string, needle: string, hint: number): number {
		let best = -1;
		let bestDist = Infinity;
		let from = 0;
		let i: number;
		while ((i = hay.indexOf(needle, from)) >= 0) {
			const d = Math.abs(i - hint);
			if (d < bestDist) {
				bestDist = d;
				best = i;
			}
			from = i + 1;
		}
		return best;
	}

	// Map the current editor selection to a source-markdown span + surrounding
	// context. In Vditor IR mode the rendered text ≈ the source for prose, so the
	// tree-walked offsets usually map straight in; we validate and fall back to a
	// proximity text-search when formatting markers skew them.
	function computeSourceSelection(): { text: string; before: string; after: string } | null {
		if (!vditorInstance || !vditorContainer) return null;
		const editorEl = vditorContainer.querySelector('.vditor-ir') as HTMLElement | null;
		if (!editorEl) return null;
		const sel = window.getSelection();
		if (!sel || sel.rangeCount === 0 || sel.isCollapsed) return null;
		const range = sel.getRangeAt(0);
		if (!editorEl.contains(range.commonAncestorContainer)) return null;
		const selText = sel.toString();
		if (!selText.trim()) return null;

		const source = vditorInstance.getValue();
		const startOff = textOffsetOf(editorEl, range.startContainer, range.startOffset);
		const endOff = textOffsetOf(editorEl, range.endContainer, range.endOffset);

		let s = -1;
		let e = -1;
		if (startOff != null && endOff != null && source.slice(startOff, endOff) === selText) {
			s = startOff;
			e = endOff;
		} else {
			s = nearestIndexOf(source, selText, startOff ?? 0);
			if (s >= 0) e = s + selText.length;
		}
		if (s < 0) return null;

		const N = 40;
		return {
			text: source.slice(s, e),
			before: source.slice(Math.max(0, s - N), s),
			after: source.slice(e, Math.min(source.length, e + N))
		};
	}

	function captureEditorSelection() {
		if (!vditorContainer) return;
		const editorEl = vditorContainer.querySelector('.vditor-ir') as HTMLElement | null;
		if (!editorEl) return;
		const sel = window.getSelection();
		if (!sel || sel.rangeCount === 0) return;
		const range = sel.getRangeAt(0);
		// Selection moved OUT of the editor (e.g. into the chat box) — keep the
		// armed selection so the user can ask about it.
		if (!editorEl.contains(range.commonAncestorContainer)) return;
		// Deliberate click/deselect inside the editor — clear it.
		if (sel.isCollapsed) {
			armedSelection = null;
			return;
		}
		const computed = computeSourceSelection();
		if (computed) {
			const words = computed.text.trim().split(/\s+/).filter(Boolean).length;
			armedSelection = { ...computed, chars: computed.text.length, words };
		}
	}

	function onSelectionChange() {
		clearTimeout(selDebounce);
		selDebounce = setTimeout(captureEditorSelection, 120);
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

	type BlockItem = {
		text: string;
		id: string | null;
		original: string;
		isFullNote?: boolean;
		sourceNoteId?: string;
		sourceNoteTitle?: string;
	};
	let allNoteBlocks = $state<BlockItem[]>([]);
	let filteredBlocks = $derived(
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

	let blockCache: Record<string, string> = {};
	let transclusionObserver: MutationObserver | null = null;

	let toolbarExpanded = $state(false);
	let toolbarNeedsToggle = $state(false);
	let toolbarResizeObserver: ResizeObserver | null = null;

	let saveStatus = $state<'saved' | 'saving' | 'unsaved'>('saved');
	let saveTimer: ReturnType<typeof setTimeout> | null = null;
	let navigationWarningDialog: HTMLDialogElement | undefined = $state();
	let deleteAttachedNoteDialog: HTMLDialogElement | undefined = $state();
	let deleteMainNoteDialog: HTMLDialogElement | undefined = $state();
	let detachPdfDialog: HTMLDialogElement | undefined = $state();

	function requestDeleteMainNote() {
		deleteMainNoteDialog?.showModal();
	}
	let pendingNavigationUrl = $state('');

	let attachPdfDialog: HTMLDialogElement | undefined = $state();
	let pdfSearchQuery = $state('');
	let pdfNotesList = $state<NoteDocument[]>([]);
	let pdfSelectedIndex = $state(0);
	let filteredPdfs = $derived(
		pdfSearchQuery.trim()
			? pdfNotesList.filter((p) => p.title.toLowerCase().includes(pdfSearchQuery.toLowerCase()))
			: pdfNotesList
	);
	let shouldRenderEditor = $derived(note !== null && (!isSourceMaterial || showAttachedNote));
	let shouldInitEditor = $derived(note !== null && (!isSourceMaterial || showAttachedNote));
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
			console.warn('Vditor destroy error:', e);
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
				invoke<SearchResponse>('search_notes', { query }).then((res) => {
					linkSearchResults = res.results.map((r) => r.note);
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
			console.error('Failed to load preview note', err);
			alert('Could not load preview.');
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

	function parseBlocks(markdown: string): BlockItem[] {
		const chunks = markdown.split(/\n+/);
		return chunks
			.map((chunk) => {
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
			})
			.filter(Boolean) as BlockItem[];
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
			console.error('Failed to load note for blocks', e);
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
								const refreshedEditorEl = vditorContainer?.querySelector(
									'.vditor-ir'
								) as HTMLElement | null;
								if (refreshedEditorEl)
									restoreSelectionTextOffset(refreshedEditorEl, selectionOffset);
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
			? globalBlocks.filter((b) => b.text.toLowerCase().includes(globalSearchQuery.toLowerCase()))
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
			console.error('Failed to load global blocks', err);
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
									const refreshedEditorEl = vditorContainer?.querySelector(
										'.vditor-ir'
									) as HTMLElement | null;
									if (refreshedEditorEl)
										restoreSelectionTextOffset(refreshedEditorEl, selectionOffset);
								}, 0);
							}
						}
					}, 50);
				}

				insertAtSavedCursor(linkText);
			} catch (err) {
				console.error('Failed to append block ID to source note', err);
				message = 'Failed to update source note.';
				setTimeout(() => (message = ''), 3000);
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
		activeSourceBytes = null;
		activeSourceId = null;
		showAttachedNote = false;

		note = await invoke<NoteDocument>('load_note', { noteId });
		const loadedNote = note;
		chatMessages = loadedNote.chatHistory || [];
		noteHistory = [];
		versionPreviewContent = null;
		activeSidebarTab = 'info';

		const relLower = loadedNote.relativePath.toLowerCase();
		isSourceMaterial = relLower.endsWith('.pdf') || relLower.endsWith('.epub') || relLower.endsWith('.html');
		
		if (isSourceMaterial) {
			sourceMaterialType = relLower.endsWith('.pdf') ? 'pdf' : relLower.endsWith('.epub') ? 'epub' : 'html';
			workingDocType = 'md';
			
			const allNotes = await invoke<NoteDocument[]>('get_all_note_documents');
			const existingScratchpad =
				allNotes
					.filter((candidate) => candidate.sourcePdf === loadedNote.id)
					.sort((left, right) => right.updatedAt.localeCompare(left.updatedAt))[0] ?? null;
			draftTitle = loadedNote.title;
			draftBody = existingScratchpad?.body ?? '';
			draftTags = loadedNote.tags.join(', ');
			activeSourceId = loadedNote.id;
			const bytes = await invoke<number[]>('read_file_binary', { noteId: loadedNote.id });
			activeSourceBytes = new Uint8Array(bytes);
			scratchpadSavedId = existingScratchpad?.id ?? null;
			showAttachedNote = draftBody.trim().length > 0;
		} else {
			workingDocType = relLower.endsWith('.tex') ? 'tex' : relLower.endsWith('.ipynb') ? 'ipynb' : 'md';
			
			draftTitle = loadedNote.title;
			draftBody = loadedNote.body;
			draftTags = loadedNote.tags.join(', ');

			if (loadedNote.sourcePdf) {
				activeSourceId = loadedNote.sourcePdf;
				const bytes = await invoke<number[]>('read_file_binary', { noteId: loadedNote.sourcePdf });
				activeSourceBytes = new Uint8Array(bytes);
				showAttachedNote = draftBody.trim().length > 0;
				// If a working document has a sourcePdf, we need to know its type. 
				// We'll query it or assume it's PDF for now unless we know otherwise.
				// (We can load it to find out)
				try {
					const sourceDoc = await invoke<NoteDocument>('load_note', { noteId: loadedNote.sourcePdf });
					const sRel = sourceDoc.relativePath.toLowerCase();
					sourceMaterialType = sRel.endsWith('.pdf') ? 'pdf' : sRel.endsWith('.epub') ? 'epub' : 'html';
				} catch (e) {
					sourceMaterialType = 'pdf'; // fallback
				}
			} else {
				activeSourceId = null;
				activeSourceBytes = null;
				sourceMaterialType = null;
				showAttachedNote = true;
			}
		}

		message = '';
		void fetchRelatedNotes();
	}

	async function refreshCurrentNoteFromBackend(skipEditorUpdate = false) {
		if (!note) return;
		const refreshed = await invoke<NoteDocument>('load_note', { noteId: note.id });
		note = {
			...refreshed,
			chatHistory: chatMessages
		};
		if (!isSourceMaterial && workingDocType === 'md') {
			draftTitle = refreshed.title;
			draftBody = refreshed.body;
			draftTags = refreshed.tags.join(', ');
			if (!skipEditorUpdate && vditorInstance && vditorInstance.getValue() !== refreshed.body) {
				vditorInstance.setValue(refreshed.body);
			}
		} else if (!isSourceMaterial) {
			draftTitle = refreshed.title;
			draftBody = refreshed.body;
			draftTags = refreshed.tags.join(', ');
		}
		void fetchRelatedNotes();
	}

	// A live note stream is starting (whole-body replace). Clear the editor so
	// the new content streams in from scratch, after stashing the old body so we
	// can restore it if the stream is cancelled.
	function beginNoteStream() {
		if (noteAnimationTimer) clearTimeout(noteAnimationTimer);
		noteAnimationTimer = undefined;
		noteStreamBackup = vditorInstance ? vditorInstance.getValue() : draftBody;
		noteStreamBuf = '';
		noteStreaming = true;
		if (vditorInstance) vditorInstance.setValue('');
	}

	// A token (or several) of the note arrived — append and reflect it live.
	function appendNoteStream(delta: string) {
		if (!noteStreaming) beginNoteStream();
		noteStreamBuf += delta;
		if (vditorInstance) vditorInstance.setValue(noteStreamBuf);
	}

	// The stream turned out not to be a whole-body replace (append/edit) — undo
	// the live preview; the authoritative note_written will apply the real change.
	function cancelNoteStream() {
		if (!noteStreaming) return;
		noteStreaming = false;
		if (vditorInstance) vditorInstance.setValue(noteStreamBackup);
	}

	// Authoritative result of a write_note tool call. Sets the final content in
	// one shot (no fake animation) and reconciles any live-streamed preview.
	function applyNoteWrite(newContent: string, mode: 'write' | 'append') {
		if (noteAnimationTimer) clearTimeout(noteAnimationTimer);
		noteAnimationTimer = undefined;
		noteStreaming = false;
		const baseContent =
			mode === 'append' && vditorInstance ? vditorInstance.getValue().trimEnd() + '\n\n' : '';
		const finalContent = baseContent + newContent;
		if (note) note = { ...note, body: finalContent };
		draftBody = finalContent;
		if (vditorInstance) vditorInstance.setValue(finalContent);
	}

	function initVditor() {
		if (!vditorContainer || vditorInstance) return;

		try {
			vditorInstance = new Vditor(vditorContainer, {
				value: draftBody,
				placeholder: isSourceMaterial ? 'Scratchpad for notes...' : 'Start typing here...',
				mode: 'ir',
				theme: 'dark',
				icon: 'material',
				lang: 'en_US',
				tab: '\t',
				cache: { enable: false },
				toolbarConfig: { pin: true },
				toolbar: [
					{
						name: 'attach-pdf',
						tipPosition: 'n',
						tip: 'Attach PDF',
						icon: '<svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"></path><polyline points="14 2 14 8 20 8"></polyline></svg>',
						click: () => {
							openAttachPdfDialog();
						}
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
								const input = linkNoteDialog?.querySelector(
									'.link-search-input'
								) as HTMLInputElement;
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
							const globalSearchBtn = vditorContainer?.querySelector(
								'button[data-type="search-blocks"]'
							) as HTMLButtonElement | null;
							if (globalSearchBtn) globalSearchBtn.click();
						} else {
							const linkBtn = vditorContainer?.querySelector(
								'button[data-type="link-note"]'
							) as HTMLButtonElement | null;
							if (linkBtn) linkBtn.click();
						}
						return;
					}
					if ((e.ctrlKey || e.metaKey) && e.shiftKey && e.key.toLowerCase() === 'z') {
						e.preventDefault();
						const redoBtn = vditorContainer?.querySelector(
							'button[data-type="redo"]'
						) as HTMLButtonElement | null;
						if (redoBtn) redoBtn.click();
					}
				},
				input: (value) => {
					draftBody = value;
					triggerAutoSave();
				}
			});
		} catch (e: any) {
			message = 'Vditor Error: ' + (e?.message || String(e));
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
		html = html.replace(
			/\[([^\]]+)\]\([^)]+\)/g,
			'<span style="color: var(--accent-200); font-weight: 500;">$1</span>'
		);
		// Strip transclusion syntax
		html = html.replace(
			/\(\([a-fA-F0-9]{6}\)\)/g,
			'<span style="color: var(--text-secondary);">(Block Link)</span>'
		);
		// Bold and italic
		html = html.replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>');
		html = html.replace(/\*([^*]+)\*/g, '<em>$1</em>');
		return html;
	}

	function scanForTransclusions() {
		if (!vditorContainer) return;
		const links = vditorContainer.querySelectorAll('[data-type="a"]:not(.transclusion-wrapper)');
		links.forEach((linkWrapper) => {
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
				invoke<NoteDocument>('load_note', { noteId: targetNoteId })
					.then((n) => {
						const blocks = parseBlocks(n.body);
						const targetBlock = blocks.find((b) => b.id === blockId);
						if (targetBlock) {
							const rawMd = targetBlock.original.replace(/\s*\(\([a-fA-F0-9]+\)\)$/, '').trim();
							let htmlText = rawMd;
							htmlText = htmlText.replace(
								/\[([^\]]+)\]\(([^)]+)\)/g,
								'<span class="mock-link">$1</span>'
							);
							htmlText = htmlText.replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>');
							htmlText = htmlText.replace(/\*([^*]+)\*/g, '<em>$1</em>');
							blockCache[cacheKey] = htmlText;
							// Set plain-text tooltip and data attribute
							const plainText = htmlText.replace(/<[^>]+>/g, '');
							(linkWrapper as HTMLElement).title = plainText;
							(linkWrapper as HTMLElement).setAttribute('data-block-content', plainText);
						}
					})
					.catch(() => {});
			}
		});
	}

	function setupTransclusionObserver() {
		if (!vditorContainer) return;
		if (transclusionObserver) transclusionObserver.disconnect();

		transclusionObserver = new MutationObserver(() => {
			scanForTransclusions();
		});

		transclusionObserver.observe(vditorContainer, {
			childList: true,
			subtree: true,
			characterData: true
		});
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
					.map((r) => r.note)
					.filter((n) => n.id !== note?.id)
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
			if (isSourceMaterial) {
				if (!scratchpadSavedId) {
					const newNote = await invoke<NoteDocument>('create_note', {
						title: draftTitle,
						sourcePdf: activeSourceId
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
				sourcePdf: activeSourceId,
				// For Source Material main notes, annotations belong to the source note, not the scratchpad
				annotations: isSourceMaterial ? [] : note.annotations
			});

			if (isSourceMaterial && note.annotations.length > 0) {
				await invoke('save_pdf_annotations', { noteId: note.id, annotations: note.annotations });
			}

			if (!isSourceMaterial) {
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
			console.error('Save error:', err);
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
		if (!note || !chatInput.trim() || isChatStreaming) return;
		const userText = chatInput.trim();
		chatInput = '';
		if (chatTextareaEl) chatTextareaEl.style.height = 'auto';
		await sendChatText(userText);
	}

	async function sendChatText(userText: string) {
		if (!note) return;
		const requestId = Date.now().toString();
		const startTime = Date.now();
		const snapshot: NoteSnapshot = {
			noteBody: draftBody,
			draftTitle: draftTitle,
			draftTags: draftTags,
			chatLength: chatMessages.length
		};
		chatMessages = [...chatMessages, { role: 'user', content: userText, snapshotId: requestId, snapshot }];
		chatMessages = [...chatMessages, { role: 'assistant', content: '', isStreaming: true, startTime }];
		setTimeout(() => scrollChatToBottom(true), 50);
		try {
			await invoke('ask_ai_stream', {
				noteId: note.id,
				question: userText,
				requestId,
				// Armed selection persists across sends (cleared only by the ✕ pill),
				// so several edits can target the same span.
				selection: armedSelection
					? { text: armedSelection.text, before: armedSelection.before, after: armedSelection.after }
					: null
			});
		} catch (e) {
			console.error('AI Error:', e);
			failStreamingChatMessage(extractChatErrorMessage(e));
		}
	}

	async function rewindToSnapshot(snapshot?: NoteSnapshot, fillInput?: string) {
		if (!snapshot || !note) return;
		if (noteAnimationTimer) {
			clearTimeout(noteAnimationTimer);
			noteAnimationTimer = undefined;
		}
		chatMessages = chatMessages.slice(0, snapshot.chatLength);
		draftBody = snapshot.noteBody;
		draftTitle = snapshot.draftTitle;
		draftTags = snapshot.draftTags;
		if (note) note = { ...note, body: snapshot.noteBody, title: snapshot.draftTitle };
		if (vditorInstance) vditorInstance.setValue(snapshot.noteBody);
		if (fillInput !== undefined) {
			chatInput = fillInput;
			await tick();
			if (chatTextareaEl) {
				chatTextareaEl.style.height = 'auto';
				chatTextareaEl.style.height = `${Math.min(chatTextareaEl.scrollHeight, 200)}px`;
				chatTextareaEl.focus();
			}
		}

		isBusy = true;
		try {
			await invoke('save_note', {
				noteId: note.id,
				title: snapshot.draftTitle,
				tags: snapshot.draftTags
					.split(',')
					.map((t: string) => t.trim())
					.filter(Boolean),
				body: snapshot.noteBody,
				sourcePdf: note.sourcePdf ?? null,
				annotations: note.annotations
			});
			await invoke('save_chat_history', { noteId: note.id, chatHistory: chatMessages });
		} catch (err) {
			console.error('Failed to rewind:', err);
		} finally {
			isBusy = false;
		}
	}

	async function retryMessage(snapshot: NoteSnapshot, userText: string) {
		await rewindToSnapshot(snapshot);
		await sendChatText(userText);
	}

	function mergeChatTools(
		existing: { name: string; details: string }[] = [],
		incoming: { name: string; details: string }[] = []
	) {
		const merged = [...existing];
		for (const tool of incoming) {
			if (!merged.some((entry) => entry.name === tool.name && entry.details === tool.details)) {
				merged.push(tool);
			}
		}
		return merged;
	}

	function wroteToCurrentNote(tools: { name: string; details: string }[] = []) {
		if (!note) return false;
		const currentTitle = note.title.trim().toLowerCase();
		return tools.some(
			(tool) =>
				(tool.name === 'Write Note' || tool.name === 'Append Note') &&
				tool.details.trim().toLowerCase() === currentTitle
		);
	}

	function finishStreamingChatMessage(tools: { name: string; details: string }[] = []) {
		chatMessages = chatMessages.map((m) => {
			if (m.isStreaming) return { ...m, isStreaming: false, endTime: Date.now() };
			return m;
		});
		if (note) invoke('save_chat_history', { noteId: note.id, chatHistory: chatMessages });
		if (wroteToCurrentNote(tools)) {
			// note_written already set the editor authoritatively — sync metadata
			// from the backend but don't overwrite the editor content.
			void refreshCurrentNoteFromBackend(true);
		}
	}

	function extractChatErrorMessage(error: unknown): string {
		if (typeof error === 'string' && error.trim()) return error;
		if (
			error &&
			typeof error === 'object' &&
			'message' in error &&
			typeof error.message === 'string' &&
			error.message.trim()
		) {
			return error.message;
		}
		return 'Failed to generate response.';
	}

	function failStreamingChatMessage(
		errorMsg: string,
		tools: { name: string; details: string }[] = []
	) {
		// If a live note stream was interrupted, the note was never saved —
		// restore the pre-stream content rather than leaving a partial draft.
		cancelNoteStream();
		chatMessages = chatMessages.map((m) => {
			if (m.isStreaming) {
				return { ...m, isStreaming: false, error: true, content: m.content + '\n\n' + errorMsg, tools, endTime: Date.now() };
			}
			return m;
		});
		if (note) invoke('save_chat_history', { noteId: note.id, chatHistory: chatMessages });
	}

	async function resolveApproval(id: string, approved: boolean) {
		chatMessages = chatMessages.map((m) => {
			if (m.isApprovalRequest && m.approvalId === id) {
				return { ...m, approvalStatus: approved ? 'approved' : 'rejected' };
			}
			return m;
		});
		await invoke('resolve_tool_approval', { id, approved });
	}

	async function fetchNoteHistory() {
		if (!note) return;
		isBusy = true;
		try {
			const history = await invoke<GitCommit[]>('get_note_history', { noteId: note.id });
			noteHistory = history
				.filter((c) => c.message && c.message.trim() !== '')
				.sort((a, b) => new Date(b.timestamp).getTime() - new Date(a.timestamp).getTime());
		} catch (e) {
			console.error('Failed to fetch history:', e);
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
			console.error('Failed to fetch version:', e);
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
			console.error('Failed to restore version:', e);
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
		const targetId = isSourceMaterial ? scratchpadSavedId : note?.sourcePdf ? note.id : null;
		const sourceId = isSourceMaterial ? activeSourceId : (note?.sourcePdf ?? activeSourceId);
		isBusy = true;
		try {
			if (targetId) {
				await invoke('delete_note', { noteId: targetId });
			}
			if (!isSourceMaterial && sourceId) {
				isProgrammaticNavigation = true;
				await goto(`/notes/${encodeURIComponent(sourceId)}`);
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

	async function openAttachPdfDialog() {
		pdfSearchQuery = '';
		pdfSelectedIndex = 0;
		isBusy = true;
		try {
			const allDocs = await invoke<NoteDocument[]>('get_all_note_documents');
			pdfNotesList = allDocs.filter((d) => d.relativePath.toLowerCase().endsWith('.pdf'));
		} catch (err) {
			message = `Failed to load PDFs: ${err}`;
		} finally {
			isBusy = false;
		}
		attachPdfDialog?.showModal();
		setTimeout(() => {
			const input = attachPdfDialog?.querySelector('.link-search-input') as HTMLInputElement | null;
			input?.focus();
		}, 50);
	}

	async function attachPdf(pdfNote: NoteDocument) {
		if (!note) return;
		attachPdfDialog?.close();
		isBusy = true;
		try {
			const saved = await invoke<NoteDocument>('save_note', {
				noteId: note.id,
				title: draftTitle,
				tags: draftTags
					.split(',')
					.map((t: string) => t.trim())
					.filter(Boolean),
				body: draftBody,
				sourcePdf: pdfNote.id,
				annotations: note.annotations
			});
			note = saved;
			activeSourceId = pdfNote.id;
			const bytes = await invoke<number[]>('read_file_binary', { noteId: pdfNote.id });
			activeSourceBytes = new Uint8Array(bytes);
			showAttachedNote = true;
			saveStatus = 'saved';
			destroyEditorInstance();
			await tick();
			initVditor();
		} catch (err) {
			message = `Failed to attach PDF: ${err}`;
		} finally {
			isBusy = false;
		}
	}

	function requestDetachPdf() {
		detachPdfDialog?.showModal();
	}

	async function confirmDetachPdf() {
		detachPdfDialog?.close();
		if (!note) return;
		isBusy = true;
		try {
			const saved = await invoke<NoteDocument>('save_note', {
				noteId: note.id,
				title: draftTitle,
				tags: draftTags
					.split(',')
					.map((t: string) => t.trim())
					.filter(Boolean),
				body: draftBody,
				sourcePdf: null,
				annotations: note.annotations
			});
			note = saved;
			activeSourceId = null;
			activeSourceBytes = null;
			saveStatus = 'saved';
			destroyEditorInstance();
			await tick();
			initVditor();
		} catch (err) {
			message = `Failed to detach PDF: ${err}`;
		} finally {
			isBusy = false;
		}
	}

	async function browseAndAttachPdf() {
		const selected = await openFileDialog({
			multiple: false,
			filters: [{ name: 'Documents', extensions: ['pdf', 'epub', 'tex', 'ipynb', 'md'] }]
		});
		if (!selected) return;
		const filePath = selected;
		attachPdfDialog?.close();
		isBusy = true;
		try {
			const pdfNote = await invoke<NoteDocument>('import_pdf_file', { filePath });
			await attachPdf(pdfNote);
		} catch (err) {
			message = `Failed to import PDF: ${err}`;
			isBusy = false;
		}
	}

	function handlePdfSearchKeydown(e: KeyboardEvent) {
		if (e.key === 'ArrowDown') {
			e.preventDefault();
			pdfSelectedIndex = Math.min(filteredPdfs.length - 1, pdfSelectedIndex + 1);
		} else if (e.key === 'ArrowUp') {
			e.preventDefault();
			pdfSelectedIndex = Math.max(0, pdfSelectedIndex - 1);
		} else if (e.key === 'Enter') {
			e.preventDefault();
			if (filteredPdfs.length > 0) attachPdf(filteredPdfs[pdfSelectedIndex]);
		}
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
			const res = await invoke<string>('extract_from_paste', {
				noteId: note.id,
				pasteContent: draftBody
			});
			const append = `\n\n> AI Extraction:\n${res}`;
			vditorInstance.insertValue(append);
			message = 'Extraction appended.';
		} finally {
			isBusy = false;
		}
	}

	let aiModal = $state<{ title: string; body: string } | null>(null);

	async function runSummarise() {
		if (!note) return;
		isBusy = true;
		try {
			message = 'Summarising note...';
			const res = await invoke<string>('summarise_note', { noteId: note.id });
			aiModal = { title: 'AI summary', body: res };
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
			aiModal = { title: 'AI answer', body: res };
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
		vditorContainer.querySelectorAll('.force-expand').forEach((el) => {
			el.classList.remove('force-expand');
		});

		// Only expand if there's an active text selection (not collapsed)
		if (!sel || sel.rangeCount === 0 || sel.isCollapsed) return;

		// Expand links that intersect the current selection
		const links = vditorContainer.querySelectorAll('[data-type="a"]');
		links.forEach((link) => {
			if (sel.containsNode(link, true)) {
				link.classList.add('force-expand');
			}
		});

		// Arm/refresh the editor selection for the AI (debounced).
		onSelectionChange();
	}

	onMount(() => {
		// Warm llama-server for as long as a note is open (stopped in onDestroy).
		noteOpened();

		const savedSidebarWidth = localStorage.getItem('myelin_sidebar_width');
		if (savedSidebarWidth) {
			const parsed = parseInt(savedSidebarWidth, 10);
			if (!isNaN(parsed)) sidebarWidth = parsed;
		}

		const timerInterval = setInterval(() => {
			currentTime = Date.now();
		}, 100);

		let unlistenChunk: UnlistenFn;
		let unlistenDone: UnlistenFn;
		let unlistenError: UnlistenFn;
		let unlistenApproval: UnlistenFn;
		let unlistenNoteWritten: UnlistenFn;
		let unlistenNoteStreamStart: UnlistenFn;
		let unlistenNoteDelta: UnlistenFn;
		let unlistenNoteStreamCancel: UnlistenFn;

		$showSidebarToggle = true;
		if (window.innerWidth > 1200) {
			$noteSidebarOpen = true;
		}

		const mql = window.matchMedia('(max-width: 1200px)');
		const handleMediaChange = (e: MediaQueryListEvent) => {
			if (e.matches) {
				$noteSidebarOpen = false;
			} else {
				$noteSidebarOpen = true;
			}
		};
		mql.addEventListener('change', handleMediaChange);
		document.addEventListener('selectionchange', handleGlobalSelectionChange);

		let unlistenTool: () => void;

		// Setup AI Streaming listeners
		listen<{ noteId: string; content: string; mode: 'write' | 'append' }>(
			'ai://note_written',
			(event) => {
				const { noteId, content, mode } = event.payload;
				if (!note || note.id !== noteId) return;
				applyNoteWrite(content, mode);
			}
		).then((fn) => (unlistenNoteWritten = fn));

		listen<{ noteId: string }>('ai://note_stream_start', (event) => {
			if (!note || note.id !== event.payload.noteId) return;
			beginNoteStream();
		}).then((fn) => (unlistenNoteStreamStart = fn));

		listen<{ noteId: string; delta: string }>('ai://note_delta', (event) => {
			if (!note || note.id !== event.payload.noteId) return;
			appendNoteStream(event.payload.delta);
		}).then((fn) => (unlistenNoteDelta = fn));

		listen<{ noteId: string }>('ai://note_stream_cancel', (event) => {
			if (!note || note.id !== event.payload.noteId) return;
			cancelNoteStream();
		}).then((fn) => (unlistenNoteStreamCancel = fn));

		listen<{ tool: string; details: string; mutatesNote?: boolean }>('ai://chat_tool', (event) => {
			let lastStartTime = Date.now();
			chatMessages = chatMessages.map((m) => {
				if (m.isStreaming) {
					lastStartTime = m.startTime || lastStartTime;
					// On a note edit, drop the model's pre-tool prose — it tends to
					// duplicate the note content that's already shown in the editor.
					return { ...m, isStreaming: false, content: event.payload.mutatesNote ? '' : m.content };
				}
				return m;
			});
			chatMessages = [
				...chatMessages,
				{
					role: 'assistant',
					content: '',
					tools: [{ name: event.payload.tool, details: event.payload.details }],
					isStreaming: false
				},
				{
					role: 'assistant',
					content: '',
					isStreaming: true,
					startTime: lastStartTime
				}
			];
			if (chatMessagesEl) {
				setTimeout(() => scrollChatToBottom(true), 100);
			}
		}).then((fn) => (unlistenTool = fn));

		listen<{ id: string; tool: string; title: string; content: string }>(
			'ai://tool_approval_request',
			(event) => {
				let lastStartTime = Date.now();
				chatMessages = chatMessages.map((m) => {
					if (m.isStreaming) {
						lastStartTime = m.startTime || lastStartTime;
						return { ...m, isStreaming: false };
					}
					return m;
				});
				chatMessages = [
					...chatMessages,
					{
						role: 'assistant',
						content: '',
						isApprovalRequest: true,
						approvalId: event.payload.id,
						approvalTool: event.payload.tool,
						approvalDetails: `Title: ${event.payload.title}\nContent:\n${event.payload.content}`,
						approvalStatus: 'pending'
					},
					{
						role: 'assistant',
						content: '',
						isStreaming: true,
						startTime: lastStartTime
					}
				];
				if (chatMessagesEl) {
					setTimeout(() => {
						scrollChatToBottom(true);
					}, 100);
				}
			}
		).then((fn) => (unlistenApproval = fn));

		listen<{ delta: string; requestId: string }>('ai://chat_chunk', (event) => {
			chatMessages = chatMessages.map((m) => {
				if (m.isStreaming) return { ...m, content: m.content + event.payload.delta };
				return m;
			});
		}).then((fn) => (unlistenChunk = fn));

		listen<{ requestId: string; tools?: { name: string; details: string }[] }>(
			'ai://chat_done',
			(event) => {
				finishStreamingChatMessage(event.payload.tools || []);
			}
		).then((fn) => (unlistenDone = fn));

		listen<{ requestId: string; message: string; tools?: { name: string; details: string }[] }>(
			'ai://chat_error',
			(event) => {
				failStreamingChatMessage(event.payload.message, event.payload.tools || []);
			}
		).then((fn) => (unlistenError = fn));

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

			clearInterval(timerInterval);

			if (unlistenChunk) unlistenChunk();
			if (unlistenDone) unlistenDone();
			if (unlistenError) unlistenError();
			if (unlistenTool) unlistenTool();
			if (unlistenApproval) unlistenApproval();
			if (unlistenNoteWritten) unlistenNoteWritten();
			if (unlistenNoteStreamStart) unlistenNoteStreamStart();
			if (unlistenNoteDelta) unlistenNoteDelta();
			if (unlistenNoteStreamCancel) unlistenNoteStreamCancel();
		};
	});

	onDestroy(() => {
		// Note view closing — stop llama-server (after a short grace) to free RAM/VRAM.
		noteClosed();
		if (noteAnimationTimer) clearTimeout(noteAnimationTimer);
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

<div
	class="editor-shell"
	class:has-attached-file={!!note?.sourcePdf || (isSourceMaterial && !!activeSourceBytes)}
>
	<header class="editor-header">
		<div class="header-copy">
			<button
				class="back-link"
				onclick={() => safeNavigate(backUrl)}
				aria-label="Go back"
				title="Go back"
			>
				<svg
					viewBox="0 0 24 24"
					width="20"
					height="20"
					stroke="currentColor"
					stroke-width="2"
					fill="none"
					stroke-linecap="round"
					stroke-linejoin="round"
				>
					<line x1="19" y1="12" x2="5" y2="12"></line>
					<polyline points="12 19 5 12 12 5"></polyline>
				</svg>
			</button>
			{#if message}
				<p class="status">{message}</p>
			{/if}
			<input
				class="title-input"
				bind:value={draftTitle}
				oninput={triggerAutoSave}
				placeholder="Note title"
			/>

			<div class="save-indicator" class:saving={saveStatus === 'saving'}>
				{#if saveStatus === 'saving'}
					<svg
						class="spinner"
						viewBox="0 0 24 24"
						width="14"
						height="14"
						stroke="currentColor"
						stroke-width="2"
						fill="none"
						stroke-linecap="round"
						stroke-linejoin="round"
						><circle cx="12" cy="12" r="10"></circle><path d="M12 6v6l4 2"></path></svg
					> Saving
				{:else if saveStatus === 'unsaved'}
					<span class="dot"></span> Unsaved
				{:else}
					<svg
						viewBox="0 0 24 24"
						width="14"
						height="14"
						stroke="currentColor"
						stroke-width="2"
						fill="none"
						stroke-linecap="round"
						stroke-linejoin="round"><polyline points="20 6 9 17 4 12"></polyline></svg
					> Saved
				{/if}
			</div>
		</div>
	</header>

	<div
		class="main-layout"
		class:split-layout={activeSourceBytes !== null && showAttachedNote}
		class:reverse-split={workingDocType === 'tex'}
		bind:this={mainLayoutEl}
	>
		{#if activeSourceBytes}
			<section class="pdf-pane" style="width: {!showAttachedNote ? '100%' : `${splitRatio}%`}">
				{#if sourceMaterialType === 'pdf'}
					<PdfViewer
						pdfBytes={activeSourceBytes}
						annotations={note?.annotations || []}
						onQuote={handlePdfQuote}
						onAnnotationsChange={handleAnnotationsChange}
						onImageExtract={handleImageExtract}
						onClosePdf={(activeSourceBytes !== null && showAttachedNote) ? requestDetachPdf : undefined}
						onAttachNote={() => {
							showAttachedNote = true;
							setTimeout(() => initVditor(), 100);
						}}
						showAttachButton={!showAttachedNote}
					/>
				{:else if sourceMaterialType === 'epub'}
					<EpubViewer epubBytes={activeSourceBytes} />
					{#if !showAttachedNote}
						<button style="position: absolute; top: 10px; right: 10px;" class="primary" onclick={() => { showAttachedNote = true; setTimeout(() => initVditor(), 100); }}>Attach Note</button>
					{/if}
				{:else if sourceMaterialType === 'html'}
					<HtmlViewer htmlBytes={activeSourceBytes} />
					{#if !showAttachedNote}
						<button style="position: absolute; top: 10px; right: 10px;" class="primary" onclick={() => { showAttachedNote = true; setTimeout(() => initVditor(), 100); }}>Attach Note</button>
					{/if}
				{/if}
			</section>
			{#if showAttachedNote}
				<!-- svelte-ignore a11y_no_static_element_interactions -->
				<div class="resizer" onmousedown={startResizing} class:resizing={isResizing}></div>
			{/if}
		{/if}

		<!-- Main Content Area -->
		{#if shouldRenderEditor}
			<section class="main-pane" style={activeSourceBytes ? `width: ${100 - splitRatio}%` : ''}>
				<div class="content-area" style="position: relative;">
					{#if workingDocType === 'md'}
						<!-- svelte-ignore a11y_click_events_have_key_events -->
						<!-- svelte-ignore a11y_no_static_element_interactions -->
						<div
							bind:this={vditorContainer}
							class="vditor-wrapper"
							class:toolbar-expanded={toolbarExpanded}
							class:has-pdf-note={!!activeSourceBytes || (!isSourceMaterial && !!note)}
							onclickcapture={handleVditorClick}
							onkeydowncapture={handleVditorKeydownCapture}
							onkeyupcapture={handleVditorKeyupCapture}
							onwheelcapture={(e) => {
								if (e.ctrlKey || e.metaKey) {
									e.preventDefault();
									e.stopPropagation();
								}
							}}
						></div>
						<div class="fullscreen-indicator">
							Press <span>{fullscreenShortcut}</span> to toggle
						</div>
					{:else if workingDocType === 'tex'}
						<div style="flex: 1; display: flex; flex-direction: column; min-height: 0;">
							<div style="padding: 4px; background: var(--bg-body); border-bottom: 1px solid var(--border-default); display: flex; justify-content: flex-end;">
								<button class="primary" onclick={async () => {
									isBusy = true;
									await saveNote();
									try {
										const pdfBytes = await invoke<number[]>('compile_latex', { noteId: note?.id });
										activeSourceBytes = new Uint8Array(pdfBytes);
										sourceMaterialType = 'pdf';
									} catch(e) {
										message = `Compile error: ${e}`;
									} finally {
										isBusy = false;
									}
								}}>Compile to PDF</button>
							</div>
							<div style="flex: 1;">
								<TexEditor
									value={draftBody}
									onInput={(val) => { draftBody = val; triggerAutoSave(); }}
								/>
							</div>
						</div>
					{:else if workingDocType === 'ipynb'}
						<IpynbEditor
							value={draftBody}
							onInput={(val) => { draftBody = val; triggerAutoSave(); }}
						/>
					{/if}

					{#if isSourceMaterial && activeSourceBytes && showAttachedNote}
						<div
							class="toolbar-close-note-container"
							style={toolbarNeedsToggle ? 'right: 50px;' : 'right: 12px;'}
						>
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
					<div class="toolbar-note-actions-container">
						{#if toolbarNeedsToggle}
							<button
								class="toolbar-overlay-toggle"
								class:expanded={toolbarExpanded}
								onclick={() => (toolbarExpanded = !toolbarExpanded)}
								aria-label="Toggle toolbar"
							>
								<svg
									viewBox="0 0 24 24"
									width="16"
									height="16"
									stroke="currentColor"
									stroke-width="2"
									fill="none"
									stroke-linecap="round"
									stroke-linejoin="round"
								>
									<polyline points="6 9 12 15 18 9"></polyline>
								</svg>
							</button>
						{/if}
						{#if activeSourceBytes !== null && showAttachedNote}
							<button
								class="toolbar-overlay-toggle"
								onclick={requestDeleteMainNote}
								aria-label="Delete Note"
								title="Delete Note"
							>
								<svg
									viewBox="0 0 24 24"
									width="14"
									height="14"
									stroke="var(--danger, #ef4444)"
									stroke-width="2"
									fill="none"
									stroke-linecap="round"
									stroke-linejoin="round"
								>
									<line x1="18" y1="6" x2="6" y2="18"></line>
									<line x1="6" y1="6" x2="18" y2="18"></line>
								</svg>
							</button>
						{/if}
					</div>
				</div>
			</section>

			<!-- Right Sidebar -->
			{#if $noteSidebarOpen}
				<!-- svelte-ignore a11y_click_events_have_key_events -->
				<!-- svelte-ignore a11y_no_static_element_interactions -->
				<div class="sidebar-backdrop" onclick={() => ($noteSidebarOpen = false)}></div>
			{/if}
			<aside
				class="sidebar"
				class:open={$noteSidebarOpen}
				style="--sidebar-width: {sidebarWidth}px;"
			>
				<!-- svelte-ignore a11y_no_static_element_interactions -->
				<div
					class="sidebar-resizer"
					onmousedown={startSidebarResizing}
					class:resizing={isSidebarResizing}
				></div>
				<div class="sidebar-tabs">
					<button
						class:active={activeSidebarTab === 'info'}
						onclick={() => (activeSidebarTab = 'info')}>Info</button
					>
					<button
						class:active={activeSidebarTab === 'chat'}
						onclick={() => (activeSidebarTab = 'chat')}>Chat</button
					>
					<button
						class:active={activeSidebarTab === 'versions'}
						onclick={() => {
							activeSidebarTab = 'versions';
							fetchNoteHistory();
						}}>History</button
					>
				</div>

				<div class="sidebar-content">
					{#if activeSidebarTab === 'info'}
						<div class="sidebar-section">
							<h3>Tags</h3>
							<input
								class="tag-input"
								bind:value={draftTags}
								oninput={triggerAutoSave}
								placeholder="comma,separated,tags"
								onblur={fetchRelatedNotes}
							/>
						</div>

						<div class="sidebar-section">
							<h3>AI Actions</h3>
							<div class="ai-actions">
								<button class="secondary" onclick={runExtract} disabled={isBusy || !note}
									>✨ Extract from paste</button
								>
								<button class="secondary" onclick={runSummarise} disabled={isBusy || !note}
									>✨ Summarise</button
								>
								<button class="secondary" onclick={runAskAI} disabled={isBusy || !note}
									>✨ Ask AI about this note</button
								>
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
											<p
												class="context-excerpt"
												style="font-size: 0.75rem; color: var(--text-secondary); margin-top: 0.25rem; line-height: 1.4;"
											>
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
							<div class="chat-messages" bind:this={chatMessagesEl} onscroll={handleChatScroll}>
								{#if chatMessages.length === 0}
									<p class="empty-state">Ask me anything about this note or your library!</p>
								{:else}
									{#each chatMessages as msg, i}
										{#if msg.role === 'user' || msg.content || (msg.tools && msg.tools.length > 0) || (msg.isApprovalRequest && msg.approvalStatus !== 'approved') || msg.isStreaming || msg.error}
											<div class="chat-message {msg.role}" class:tool-only={!msg.content && ((msg.tools && msg.tools.length > 0) || msg.isApprovalRequest)}>
												<div class="chat-bubble" class:error={msg.error}>
													{#if msg.tools && msg.tools.length > 0}
														<div class="chat-tools">
															{#each msg.tools as tool}
																<ChatToolIndicator {tool} />
															{/each}
														</div>
													{/if}
													{#if msg.error}
														<span class="chat-error-text">
															{msg.content || 'Failed to generate response.'}
														</span>
													{:else if msg.isApprovalRequest && msg.approvalStatus !== 'approved'}
														<ChatToolIndicator tool={{ name: (msg.approvalStatus === 'rejected' ? 'Rejected tool: ' : 'Pending tool: ') + msg.approvalTool, details: msg.approvalDetails || '' }} />
													{:else if msg.role === 'assistant' && msg.content}
														{@html DOMPurify.sanitize(marked.parse(msg.content) as string, { ADD_TAGS: ['think'] })}
													{:else if msg.content}
														{msg.content}
													{/if}
													{#if msg.isStreaming && msg.startTime}
														{#if !msg.content}
									<span class="chat-working" aria-label="Working"><span></span><span></span><span></span></span>
								{/if}
								<span class="chat-time-taken live">{((currentTime - msg.startTime) / 1000).toFixed(1)}s</span>
													{:else if msg.endTime && msg.startTime}
														<span class="chat-time-taken">{((msg.endTime - msg.startTime) / 1000).toFixed(1)}s</span>
													{/if}
												</div>
												{#if msg.role === 'user' && msg.snapshot}
													<div class="chat-msg-actions">
														<button
															class="rewind-btn"
															onclick={() => rewindToSnapshot(msg.snapshot, msg.content)}
															title="Undo — restore note and put prompt back in input">↩</button
														>
														<button
															class="rewind-btn retry"
															onclick={() => retryMessage(msg.snapshot!, msg.content)}
															title="Retry — rewind and resend this prompt">↻</button
														>
													</div>
												{/if}
											</div>
										{/if}
									{/each}
								{/if}
							</div>
							
							{#if chatMessages.find(m => m.isApprovalRequest && m.approvalStatus === 'pending')}
								{@const pendingReq = chatMessages.find(m => m.isApprovalRequest && m.approvalStatus === 'pending')}
								<div class="pending-approval-bar">
									<div class="pending-info">
										<span class="tool-icon">⚡</span>
										<span class="pending-text">AI wants to use <strong>{pendingReq?.approvalTool}</strong></span>
									</div>
									<div class="pending-actions">
										<button class="primary" onclick={() => resolveApproval(pendingReq!.approvalId!, true)}>Approve</button>
										<button class="secondary" onclick={() => resolveApproval(pendingReq!.approvalId!, false)}>Reject</button>
									</div>
								</div>
							{/if}
							
							<div class="chat-input-area">
								<div class="prompt-box">
									<textarea
										bind:this={chatTextareaEl}
										bind:value={chatInput}
										onkeydown={(e) => {
											if (e.key === 'Enter' && !e.shiftKey) {
												e.preventDefault();
												if (chatInput.trim() && !isChatStreaming) sendChatMessage();
											}
										}}
										oninput={(e) => {
											const target = e.target as HTMLTextAreaElement;
											target.style.height = 'auto';
											target.style.height = `${Math.min(target.scrollHeight + 2, 150)}px`;
										}}
										placeholder="Ask AI…"
										rows="1"
									></textarea>
									<div class="prompt-toolbar">
										<button
											type="button"
											class="mode-pill"
											class:auto={!requireToolApproval}
											onclick={() => {
												requireToolApproval = !requireToolApproval;
												invoke('set_require_tool_approval', { require: requireToolApproval });
											}}
											title={requireToolApproval
												? 'Ask: confirms before each tool action — click for Auto'
												: 'Auto: edits freely without asking — click to require permission'}
										>
											{requireToolApproval ? 'Ask' : 'Auto'}
										</button>
										<button
											type="button"
											class="prompt-icon-btn"
											onclick={attachFile}
											title="Attach a file"
											aria-label="Attach a file"
										>
											<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><line x1="12" y1="5" x2="12" y2="19"/><line x1="5" y1="12" x2="19" y2="12"/></svg>
										</button>
										{#if armedSelection}
												<button
													type="button"
													class="selection-pill"
													onclick={() => (armedSelection = null)}
													title={`The AI will edit only your selection — ${armedSelection.chars} chars, ${armedSelection.words} word${armedSelection.words === 1 ? '' : 's'}. Click to clear.`}
												>
													<svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M4 7V5a1 1 0 0 1 1-1h2M17 4h2a1 1 0 0 1 1 1v2M20 17v2a1 1 0 0 1-1 1h-2M7 20H5a1 1 0 0 1-1-1v-2"/></svg>
													<span>{armedSelection.chars} sel</span>
													<span class="sel-x">✕</span>
												</button>
											{/if}
											<div class="prompt-spacer"></div>
										<div class="context-ring" title={`~${contextPercent}% of the context window used`}>
											<svg viewBox="0 0 36 36" width="20" height="20" aria-hidden="true">
												<circle class="ring-track" cx="18" cy="18" r="15.5"></circle>
												<circle class="ring-value" cx="18" cy="18" r="15.5" style={`stroke-dasharray:${RING_CIRC};stroke-dashoffset:${ringOffset};stroke:${ringColor};`}></circle>
											</svg>
										</div>
										<button
											type="button"
											class="send-btn"
											onclick={() => { if (chatInput.trim() && !isChatStreaming) sendChatMessage(); }}
											disabled={!chatInput.trim() || isChatStreaming}
											aria-label="Send"
											title={isChatStreaming ? 'Waiting for the current reply to finish…' : 'Send (Enter)'}
										>
											<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><line x1="12" y1="19" x2="12" y2="5"/><polyline points="5 12 12 5 19 12"/></svg>
										</button>
									</div>
								</div>
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
												<span class="commit-date"
													>{new Date(commit.timestamp).toLocaleString()}</span
												>
											</div>
											<div class="commit-actions">
												<button class="secondary" onclick={() => previewVersion(commit.hash)}
													>Preview</button
												>
												<button class="secondary" onclick={() => restoreVersion(commit.hash)}
													>Restore</button
												>
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

<dialog
	bind:this={versionPreviewDialog}
	class="version-preview-dialog"
	onclose={() => (versionPreviewContent = null)}
>
	<div class="dialog-content" style="max-width: 800px; width: 90vw;">
		<h3>Version Preview</h3>
		<div
			class="preview-content"
			style="max-height: 60vh; overflow-y: auto; background: var(--bg-page); padding: 1rem; border-radius: var(--radius-sm); border: 1px solid var(--border-default); white-space: pre-wrap; font-family: var(--font-mono); font-size: 0.875rem; margin: 1rem 0;"
		>
			{versionPreviewContent || 'Loading...'}
		</div>
		<div class="dialog-actions">
			<button class="secondary" onclick={() => versionPreviewDialog?.close()}>Close</button>
			{#if versionPreviewHash}
				<button class="primary" onclick={() => restoreVersion(versionPreviewHash!)}
					>Restore This Version</button
				>
			{/if}
		</div>
	</div>
</dialog>

<dialog bind:this={mathDialog} class="math-dialog" onclose={() => (mathValue = '')}>
	<div class="dialog-content">
		<h3>Insert Math</h3>
		<div class="math-container">
			<svelte:element
				this={'math-field'}
				oninput={(e: any) => (mathValue = e.target.value)}
				style="width: 100%; font-size: 1.5rem; padding: 0.5rem; background: var(--bg-panel); color: var(--text-primary); border: 1px solid var(--border-default); border-radius: var(--radius-xs);"
				>{mathValue}</svelte:element
			>
		</div>
		<div class="dialog-actions">
			<button class="secondary" onclick={() => mathDialog?.close()}>Cancel</button>
			<button class="primary" onclick={insertMath} disabled={!mathValue}>Insert</button>
		</div>
	</div>
</dialog>

<dialog
	bind:this={linkNoteDialog}
	class="link-dialog"
	onkeydown={handleLinkSearchKeydown}
	onclose={() => {
		linkSearchQuery = '';
		linkSearchResults = [];
		linkSelectedIndex = 0;
		linkDialogMode = 'notes';
		if (shouldRefocusEditor) refocusEditorSoon();
	}}
>
	<div class="dialog-content">
		{#if linkDialogMode === 'notes'}
			<h3>Link to Note</h3>
			<p style="font-size: 0.875rem; color: var(--text-secondary); margin-bottom: var(--space-4);">
				Search and select a note to link your highlighted text to.
			</p>

			<input
				class="link-search-input"
				bind:value={linkSearchQuery}
				oninput={() => (linkSelectedIndex = 0)}
				use:autofocus
				placeholder="Search notes..."
			/>

			{#if linkSearchQuery.trim() || linkSearchResults.length > 0}
				<div class="link-results-container">
					{#if linkSearchResults.length > 0}
						<ul class="link-results-list">
							{#each linkSearchResults as res, i (res.id + '_' + i)}
								<li>
									<button
										class="link-result-btn"
										class:selected={i === linkSelectedIndex}
										onclick={() => selectNoteForBlocks(res)}
									>
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
			<p style="font-size: 0.875rem; color: var(--text-secondary); margin-bottom: var(--space-4);">
				Select a specific block from <strong>{selectedNoteForBlocks?.title}</strong> or link the entire
				note.
			</p>

			<input
				class="link-search-input"
				bind:value={linkSearchQuery}
				oninput={() => (linkSelectedIndex = 0)}
				use:autofocus
				placeholder="Search blocks..."
			/>

			<div class="link-results-container">
				{#if filteredBlocks.length > 0}
					<ul class="link-results-list">
						{#each filteredBlocks as block, i}
							<li>
								<button
									class="link-result-btn"
									class:selected={i === linkSelectedIndex}
									onclick={() => insertBlockLink(block)}
								>
									<span
										style={block.isFullNote
											? 'font-weight: bold;'
											: 'font-size: 0.9em; opacity: 0.9; display: -webkit-box; -webkit-line-clamp: 2; -webkit-box-orient: vertical; overflow: hidden;'}
									>
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
				<button
					class="secondary"
					style="margin-right: auto;"
					onclick={() => {
						linkDialogMode = 'notes';
						linkSearchQuery = '';
						linkSelectedIndex = 0;
					}}>Back</button
				>
			{/if}
			<button class="secondary" onclick={() => linkNoteDialog?.close()}>Cancel</button>
		</div>
	</div>
</dialog>

<dialog
	bind:this={globalSearchDialog}
	class="link-dialog"
	onkeydown={handleGlobalSearchKeydown}
	onclose={() => {
		globalSearchQuery = '';
		globalSelectedIndex = 0;
		globalBlocks = [];
		if (shouldRefocusEditor) refocusEditorSoon();
	}}
>
	<div class="dialog-content">
		<h3>Search Global Blocks</h3>
		<p style="font-size: 0.875rem; color: var(--text-secondary); margin-bottom: var(--space-4);">
			Search blocks across all notes.
		</p>

		<input
			class="link-search-input"
			bind:value={globalSearchQuery}
			oninput={() => (globalSelectedIndex = 0)}
			placeholder="Search global blocks..."
		/>

		<div class="link-results-container">
			{#if filteredGlobalBlocks.length > 0}
				<ul class="link-results-list">
					{#each filteredGlobalBlocks as block, i}
						<li>
							<button
								class="link-result-btn"
								class:selected={i === globalSelectedIndex}
								onclick={() => insertGlobalBlockLink(block)}
							>
								<div>
									<span
										style="font-size: 0.9em; opacity: 0.9; display: -webkit-box; -webkit-line-clamp: 2; -webkit-box-orient: vertical; overflow: hidden; text-align: left;"
									>
										{block.text}
									</span>
									<span
										style="font-size: 0.7em; opacity: 0.6; display: block; margin-top: 2px; text-align: left;"
									>
										From: {block.sourceNoteTitle}
									</span>
								</div>
							</button>
						</li>
					{/each}
				</ul>
			{:else}
				<p class="empty-state">
					{globalBlocks.length > 0 ? 'No matching blocks found.' : 'Loading blocks...'}
				</p>
			{/if}
		</div>

		<div class="dialog-actions">
			<button class="secondary" onclick={() => globalSearchDialog?.close()}>Cancel</button>
		</div>
	</div>
</dialog>

<dialog
	bind:this={previewNoteDialog}
	class="preview-dialog"
	onclose={() => {
		previewNoteTarget = null;
	}}
>
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
					<div
						bind:this={previewNoteContainer}
						class="vditor-reset"
						style="padding: 2rem; min-height: 100%;"
					></div>
				</div>
			</div>
			<div class="preview-sidebar">
				<button class="icon-btn" onclick={() => previewNoteDialog?.close()} title="Close Preview">
					<svg
						viewBox="0 0 24 24"
						width="24"
						height="24"
						stroke="currentColor"
						stroke-width="2"
						fill="none"
						stroke-linecap="round"
						stroke-linejoin="round"
						><line x1="18" y1="6" x2="6" y2="18"></line><line x1="6" y1="6" x2="18" y2="18"
						></line></svg
					>
				</button>
				<button class="icon-btn" onclick={expandPreviewNoteDirect} title="Expand Note">
					<svg
						viewBox="0 0 24 24"
						width="24"
						height="24"
						stroke="currentColor"
						stroke-width="2"
						fill="none"
						stroke-linecap="round"
						stroke-linejoin="round"
						><path d="M15 3h6v6"></path><path d="M9 21H3v-6"></path><path d="M21 3l-7 7"
						></path><path d="M3 21l7-7"></path></svg
					>
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

<dialog
	bind:this={deleteAttachedNoteDialog}
	class="dialog math-dialog"
	onclose={cancelDeleteAttachedNote}
>
	<div class="dialog-content">
		<h3 style="margin-top: 0;">Delete Attached Note</h3>
		<p style="color: var(--text-secondary); margin-bottom: var(--space-6);">
			All data and annotations in this attached note will be deleted permanently, and the note pane
			will be closed.
		</p>
		<div class="dialog-actions">
			<button class="secondary" onclick={cancelDeleteAttachedNote}>Cancel</button>
			<button class="danger" onclick={confirmDeleteAttachedNote} disabled={isBusy}
				>Delete Note</button
			>
		</div>
	</div>
</dialog>

<dialog bind:this={deleteMainNoteDialog} class="dialog math-dialog">
	<div class="dialog-content">
		<h3 style="margin-top: 0;">Delete Note</h3>
		<p style="color: var(--text-secondary); margin-bottom: var(--space-6);">
			This note will be permanently deleted. This action cannot be undone.
		</p>
		<div class="dialog-actions">
			<button class="secondary" onclick={() => deleteMainNoteDialog?.close()}>Cancel</button>
			<button
				class="danger"
				onclick={() => {
					deleteMainNoteDialog?.close();
					deleteNote();
				}}
				disabled={isBusy}>Delete Note</button
			>
		</div>
	</div>
</dialog>

<dialog bind:this={detachPdfDialog} class="dialog math-dialog">
	<div class="dialog-content">
		<h3 style="margin-top: 0;">Close PDF</h3>
		<p style="color: var(--text-secondary); margin-bottom: var(--space-6);">
			The PDF will be detached from this note. You can re-attach it at any time.
		</p>
		<div class="dialog-actions">
			<button class="secondary" onclick={() => detachPdfDialog?.close()}>Cancel</button>
			<button class="danger" onclick={confirmDetachPdf} disabled={isBusy}>Close PDF</button>
		</div>
	</div>
</dialog>

<dialog
	bind:this={attachPdfDialog}
	class="pdf-attach-dialog"
	onclose={() => {
		pdfSearchQuery = '';
		pdfSelectedIndex = 0;
	}}
>
	<div class="dialog-content">
		<h3>Attach a file</h3>
		<p class="dialog-subtitle">Select a PDF from your workspace or upload a new one.</p>

		<input
			class="link-search-input"
			bind:value={pdfSearchQuery}
			oninput={() => (pdfSelectedIndex = 0)}
			placeholder="Search PDFs..."
		/>

		<div class="pdf-grid-container">
			<button class="pdf-grid-upload-card" onclick={browseAndAttachPdf} disabled={isBusy}>
				<div class="upload-icon-wrapper">
					<svg
						viewBox="0 0 24 24"
						width="32"
						height="32"
						stroke="currentColor"
						stroke-width="1.5"
						fill="none"
						stroke-linecap="round"
						stroke-linejoin="round"
						><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"></path><polyline
							points="17 8 12 3 7 8"
						></polyline><line x1="12" y1="3" x2="12" y2="15"></line></svg
					>
				</div>
				<span class="upload-text">Upload new file</span>
				<span class="upload-subtext">Choose a PDF from your computer</span>
			</button>

			{#each filteredPdfs as pdf, i (pdf.id)}
				<button class="pdf-grid-card" onclick={() => attachPdf(pdf)}>
					<div class="pdf-card-icon">
						<svg
							viewBox="0 0 24 24"
							width="24"
							height="24"
							stroke="var(--accent-300)"
							stroke-width="1.5"
							fill="none"
							stroke-linecap="round"
							stroke-linejoin="round"
							><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"></path><polyline
								points="14 2 14 8 20 8"
							></polyline></svg
						>
					</div>
					<div class="pdf-card-info">
						<strong>{pdf.title}</strong>
						<span>{new Date(pdf.createdAt).toLocaleDateString()}</span>
					</div>
				</button>
			{/each}
		</div>

		<div class="dialog-actions">
			<button class="secondary" onclick={() => attachPdfDialog?.close()}>Cancel</button>
		</div>
	</div>
</dialog>

{#if aiModal}
	<div class="ai-modal-overlay" role="presentation" onclick={() => (aiModal = null)}>
		<div class="ai-modal" role="dialog" aria-modal="true" onclick={(e) => e.stopPropagation()}>
			<h3 class="ai-modal-title">{aiModal.title}</h3>
			<div class="ai-modal-body">{aiModal.body}</div>
			<div class="dialog-actions">
				<button class="secondary" onclick={() => (aiModal = null)}>Close</button>
			</div>
		</div>
	</div>
{/if}

<style>
	.ai-modal-overlay {
		position: fixed;
		inset: 0;
		background: rgba(0, 0, 0, 0.5);
		display: flex;
		align-items: center;
		justify-content: center;
		z-index: 1000;
		padding: var(--space-4);
	}
	.ai-modal {
		background: var(--bg-elevated, #1a1a1a);
		border: 1px solid var(--border-default);
		border-radius: var(--radius-md, 8px);
		max-width: 640px;
		width: 100%;
		max-height: 80vh;
		display: flex;
		flex-direction: column;
		padding: var(--space-4);
		gap: var(--space-3);
	}
	.ai-modal-title {
		margin: 0;
		font-size: 0.95rem;
		color: var(--text-secondary);
		font-weight: 600;
	}
	.ai-modal-body {
		overflow-y: auto;
		white-space: pre-wrap;
		line-height: 1.5;
		color: var(--text-primary);
		font-size: 0.9rem;
	}

	:global(.chat-bubble think) {
		display: block;
		padding: 12px 14px;
		margin: 8px 0;
		border-left: 3px solid var(--accent, #f37021);
		color: rgba(255, 255, 255, 0.6);
		font-style: italic;
		background: rgba(0, 0, 0, 0.2);
		border-radius: 4px;
		font-size: 0.9em;
	}
	:global(.chat-bubble think::before) {
		content: '💭 Thinking Process';
		display: block;
		font-weight: 600;
		font-style: normal;
		margin-bottom: 6px;
		color: rgba(255, 255, 255, 0.8);
		font-size: 0.85rem;
		text-transform: uppercase;
		letter-spacing: 0.05em;
	}

	.editor-shell {
		height: 100%;
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
	.title-input:hover,
	.title-input:focus {
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
		100% {
			transform: rotate(360deg);
		}
	}
	button:disabled {
		opacity: 0.6;
		cursor: not-allowed;
	}

	.main-layout {
		flex: 1;
		min-height: 0;
		position: relative;
		display: flex;
		overflow: hidden;
		z-index: 20; /* Ensures tooltips render above the header's stacking context */
	}

	.main-layout.reverse-split .pdf-pane {
		order: 2;
	}
	.main-layout.reverse-split .resizer {
		order: 1;
	}
	.main-layout.reverse-split .main-pane {
		order: 0;
	}
	.main-layout.reverse-split .sidebar {
		order: 3;
	}

	.pdf-pane {
		min-width: 26rem;
	}

	.resizer {
		width: 10px;
		flex: 0 0 10px;
		cursor: col-resize;
		position: relative;
		background: linear-gradient(
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
		background: linear-gradient(
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
		min-width: 800px;
		display: flex;
		flex-direction: column;
		background: var(--bg-page);
		align-items: stretch;
		min-height: 0;
		overflow-y: auto; /* Make main pane the scroll container */
		overflow-x: hidden;
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
		display: flex;
		flex-direction: column;
		flex: 1;
		min-height: 0;
	}

	.vditor-wrapper {
		border: none !important;
		flex: 1;
		min-height: 0;
	}

	:global(.vditor) {
		height: 100% !important;
	}

	:global(.vditor-reset) {
		padding-top: var(--space-6) !important;
	}

	.toolbar-note-actions-container {
		position: absolute;
		top: 0;
		right: var(--space-6);
		height: 48px;
		display: flex;
		align-items: center;
		justify-content: center;
		gap: var(--space-2);
		z-index: 40;
	}

	.toolbar-close-note-btn {
		pointer-events: auto;
		border: 1px solid var(--border-subtle);
		background: var(--bg-panel);
		color: currentColor;
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

	.toolbar-attach-pdf-btn {
		pointer-events: auto;
		display: flex;
		align-items: center;
		gap: 0.375rem;
		border: 1px solid var(--border-subtle);
		background: var(--bg-panel);
		color: var(--text-secondary);
		border-radius: var(--radius-sm);
		padding: 0.4rem 0.75rem;
		height: 32px;
		font-size: 0.82rem;
		font-family: var(--font-mono);
		line-height: 1;
		white-space: nowrap;
		cursor: pointer;
		transition: all var(--duration-fast);
	}

	.toolbar-attach-pdf-btn:hover:not(:disabled) {
		border-color: var(--accent-200);
		color: var(--accent-100);
	}

	.toolbar-overlay-toggle {
		width: 28px;
		height: 28px;
		display: flex;
		align-items: center;
		justify-content: center;
		background: var(--bg-surface);
		border: 1px solid var(--border-subtle);
		border-radius: var(--radius-sm);
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

	/* Clear padding from the scroll container so its scrollbar is pinned to the far right edge */
	:global(.vditor-ir) {
		padding: 0 !important;
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

	/* Fixed A4 width for the text container, centered horizontally.
	   This makes the width strictly static across screen sizes. */
	:global(.vditor-reset) {
		width: 210mm !important;
		max-width: none !important;
		margin: 0 auto !important;
		padding-left: var(--space-8) !important;
		padding-right: var(--space-8) !important;
		overflow: visible !important;
		box-sizing: border-box !important;
	}

	:global(.vditor-preview__action) {
		display: none !important;
	}

	@media (min-width: 1200px) {
		:global(.vditor-content:has(.vditor-sv[style*='block'])) {
			flex-direction: row !important;
			align-items: stretch !important;
			justify-content: center !important;
			gap: 0 !important;
			padding: 0 !important;
		}

		:global(.vditor-content:has(.vditor-sv[style*='block']) .vditor-ir),
		:global(.vditor-content:has(.vditor-sv[style*='block']) .vditor-sv),
		:global(.vditor-content:has(.vditor-sv[style*='block']) .vditor-preview) {
			margin: 0 !important;
		}

		:global(.vditor-content:has(.vditor-sv[style*='block']) .vditor-reset) {
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
		padding-right: 120px !important;
		transition: max-height 0.2s ease-out;
		position: relative !important;
		z-index: 30 !important;
	}

	:global(.vditor-wrapper.has-pdf-note .vditor-toolbar) {
		padding-right: 120px !important;
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
		transition:
			transform 0.3s cubic-bezier(0.16, 1, 0.3, 1),
			margin-right 0.3s cubic-bezier(0.16, 1, 0.3, 1);
		border-left: 1px solid var(--border-default);
		border-radius: 0 !important;
		box-shadow: -4px 0 24px rgba(0, 0, 0, 0.4);
		font-family: var(--font-mono);
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

	/* Large Screen — sidebar docks side by side with the editor */
	@media (min-width: 1201px) {
		.sidebar {
			position: relative;
			transform: none;
			margin-right: calc(var(--sidebar-width, 20rem) * -1);
			/* Hard cap relative to the layout container (excludes the left rail), so
			   the note keeps its 800px min-width and the panes never overflow/clip. */
			max-width: calc(100% - 800px);
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
	.sidebar-resizer:hover,
	.sidebar-resizer.resizing {
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
		color: var(--text-primary);
	}

	.row-badge {
		border: 1px solid var(--border-default);
		border-radius: 4px;
		padding: 2px 6px;
		font-size: 0.65rem;
		color: var(--neutral-400);
		font-family: var(--font-mono);
		background: rgba(255, 255, 255, 0.02);
		flex-shrink: 0;
		white-space: nowrap;
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
	.tag-input:focus {
		border-color: var(--accent-200);
	}

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
		from {
			opacity: 0;
			transform: translateY(8px);
		}
		to {
			opacity: 1;
			transform: translateY(0);
		}
	}

	@media (max-width: 1024px) {
		.editor-header {
			flex-wrap: wrap;
			gap: var(--space-4);
			position: sticky;
			top: 0;
			z-index: 10;
		}
		.title-input {
			max-width: 100%;
		}
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
	.link-search-input:focus {
		border-color: var(--accent-200);
	}

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

	:global(.has-attached-file [data-type='attach-pdf']) {
		opacity: 0.3 !important;
		pointer-events: none !important;
		cursor: not-allowed !important;
	}

	.pdf-attach-dialog {
		width: 100%;
		max-width: 800px;
		background: var(--bg-modal);
		border: 1px solid var(--border-subtle);
		border-radius: var(--radius-md);
		color: var(--text-primary);
		box-shadow: 0 10px 30px rgba(0, 0, 0, 0.5);
	}
	.pdf-attach-dialog::backdrop {
		background: rgba(0, 0, 0, 0.6);
		backdrop-filter: blur(2px);
	}
	.pdf-attach-dialog .dialog-content {
		padding: var(--space-6);
	}
	.pdf-attach-dialog h3 {
		margin-top: 0;
		font-size: 1.25rem;
		font-weight: 500;
	}
	.dialog-subtitle {
		font-size: 0.875rem;
		color: var(--text-secondary);
		margin-bottom: var(--space-4);
	}

	.pdf-grid-container {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(180px, 1fr));
		gap: 16px;
		margin-top: 12px;
		margin-bottom: 24px;
		max-height: 400px;
		overflow-y: auto;
		padding: 4px;
		padding-right: 8px;
	}

	.pdf-grid-container::-webkit-scrollbar {
		width: 6px;
	}
	.pdf-grid-container::-webkit-scrollbar-thumb {
		background: var(--border-default);
		border-radius: 4px;
	}

	.pdf-grid-upload-card {
		background: rgba(255, 255, 255, 0.02);
		border: 1px dashed var(--border-default);
		border-radius: var(--radius-sm);
		padding: 24px 16px;
		display: flex;
		flex-direction: column;
		align-items: center;
		justify-content: center;
		gap: 8px;
		cursor: pointer;
		transition: all 0.2s ease;
		color: var(--text-primary);
		text-align: center;
		font-family: inherit;
	}
	.pdf-grid-upload-card:hover:not(:disabled) {
		background: rgba(255, 255, 255, 0.05);
		border-color: var(--accent-300);
	}
	.pdf-grid-upload-card:disabled {
		opacity: 0.5;
		cursor: not-allowed;
	}
	.upload-icon-wrapper {
		color: var(--accent-300);
		margin-bottom: 4px;
	}
	.upload-text {
		font-weight: 500;
		font-size: 0.95rem;
	}
	.upload-subtext {
		font-size: 0.75rem;
		color: var(--text-secondary);
	}

	.pdf-grid-card {
		background: rgba(255, 255, 255, 0.03);
		border: 1px solid var(--border-subtle);
		border-radius: var(--radius-sm);
		padding: 16px;
		display: flex;
		flex-direction: column;
		align-items: flex-start;
		gap: 12px;
		cursor: pointer;
		transition: all 0.2s ease;
		text-align: left;
		font-family: inherit;
		color: inherit;
	}
	.pdf-grid-card:hover {
		background: rgba(255, 255, 255, 0.06);
		border-color: var(--border-default);
		transform: translateY(-2px);
	}
	.pdf-card-icon {
		background: rgba(0, 0, 0, 0.2);
		padding: 12px;
		border-radius: 8px;
		display: flex;
		align-items: center;
		justify-content: center;
	}
	.pdf-card-info {
		display: flex;
		flex-direction: column;
		gap: 4px;
		width: 100%;
	}
	.pdf-card-info strong {
		font-size: 0.9rem;
		font-weight: 500;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
		width: 100%;
	}
	.pdf-card-info span {
		font-size: 0.75rem;
		color: var(--text-secondary);
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
		box-shadow: 0 12px 48px rgba(0, 0, 0, 0.5);
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
	:global(
		.transclusion-wrapper[data-block-content]:not([data-block-content='']):not(
				.vditor-ir__node--expand
			):not(.force-expand)
			.vditor-ir__link
	) {
		display: none !important;
	}
	/* Strip the wrapper pill background when collapsed because we only want the ::after to show */
	:global(
		.transclusion-wrapper[data-block-content]:not([data-block-content='']):not(
				.vditor-ir__node--expand
			):not(.force-expand)
	) {
		padding: 0 !important;
		background: transparent !important;
		border: none !important;
		display: block !important;
	}
	/* Ensure the ::after preview has no top margin since there's no text above it */
	:global(
		.transclusion-wrapper[data-block-content]:not([data-block-content='']):not(
				.vditor-ir__node--expand
			):not(.force-expand)::after
	) {
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
	:global(.vditor-ir__node[data-type='a'].force-expand .vditor-ir__marker),
	:global(.vditor-ir__node[data-type='a'].vditor-ir__node--expand .vditor-ir__marker) {
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

	:global(.vditor-hint button[data-mode='wysiwyg']) {
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
	.sidebar-content,
	.chat-messages,
	.versions-container,
	.sidebar {
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
	.chat-message.tool-only {
		margin-top: calc(-1 * var(--space-3));
		margin-bottom: calc(-1 * var(--space-3));
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
		min-width: 0;
		word-break: break-word;
		overflow-wrap: anywhere;
	}
	.chat-message.user .chat-bubble {
		background: var(--accent-200);
		color: var(--bg-page);
	}
	.chat-message.assistant .chat-bubble {
		background: var(--bg-panel);
		color: var(--text-primary);
	}
	.chat-message.tool-only .chat-bubble {
		padding-top: var(--space-1);
		padding-bottom: var(--space-1);
		background: transparent;
	}
	.chat-bubble.error {
		background: color-mix(in srgb, var(--bg-panel) 82%, var(--accent-200));
	}
	.chat-bubble.error {
		border-left: 3px solid var(--error-color, #e53e3e);
		background-color: var(--error-bg, rgba(229, 62, 62, 0.1));
	}
	
	.approval-card {
		background: rgba(0, 0, 0, 0.2);
		border: 1px solid var(--accent);
		border-radius: var(--radius-md);
		padding: var(--space-3);
		margin: var(--space-2) 0;
		display: flex;
		flex-direction: column;
		gap: var(--space-2);
		width: 100%;
		box-sizing: border-box;
	}
	.approval-card.rejected {
		border-color: rgba(239, 68, 68, 0.4);
		background: rgba(239, 68, 68, 0.05);
	}
	.approval-card.rejected .title {
		color: #fca5a5;
	}
	
	.approval-card .title {
		margin: 0;
		font-size: 0.85rem;
		color: var(--accent-300);
	}
	
	.approval-card pre {
		background: rgba(0, 0, 0, 0.3);
		padding: var(--space-2);
		border-radius: var(--radius-sm);
		font-size: 0.75rem;
		font-family: var(--font-mono);
		white-space: pre-wrap;
		word-break: break-word;
		max-height: 120px;
		overflow-y: auto;
		margin: 0;
		color: var(--text-muted);
	}
	
	.approval-card pre::-webkit-scrollbar {
		width: 6px;
		height: 6px;
	}
	.approval-card pre::-webkit-scrollbar-track {
		background: transparent;
	}
	.approval-card pre::-webkit-scrollbar-thumb {
		background: var(--neutral-700, #444);
		border-radius: 3px;
	}
	.approval-card pre::-webkit-scrollbar-thumb:hover {
		background: var(--neutral-500, #666);
	}
	
	.approval-actions {
		display: flex;
		gap: var(--space-2);
		flex-wrap: wrap; /* Fix responsiveness for narrow sidebars */
	}
	
	.pending-approval-bar {
		display: flex;
		justify-content: space-between;
		align-items: center;
		padding: var(--space-3) var(--space-4);
		background: var(--surface-2);
		border-top: 1px solid var(--border-subtle);
		border-bottom: 1px solid var(--border-subtle);
		gap: var(--space-4);
	}
	.pending-info {
		display: flex;
		align-items: center;
		gap: var(--space-2);
		font-size: 0.85rem;
		color: var(--text-primary);
	}
	.pending-actions {
		display: flex;
		gap: var(--space-2);
	}
	.pending-actions button {
		padding: 0.4rem 0.8rem;
		border-radius: var(--radius-sm);
		font-size: 0.8rem;
		cursor: pointer;
		font-weight: 500;
	}
	.pending-actions .primary {
		background: var(--accent-500);
		color: white;
		border: 1px solid var(--accent-500);
	}
	.pending-actions .secondary {
		background: transparent;
		color: var(--text-primary);
		border: 1px solid var(--border-default);
	}
	.chat-tools {
		display: flex;
		flex-direction: column;
		gap: var(--space-2);
		margin-bottom: var(--space-2);
	}
	.chat-tools.streaming {
		position: sticky;
		top: 0;
		z-index: 1;
		background: inherit;
		padding-bottom: var(--space-2);
	}
	.chat-input-area {
		display: flex;
		flex-direction: column;
		gap: var(--space-2);
		padding-top: var(--space-3);
		border-top: 1px solid var(--border-subtle);
	}
	.prompt-box {
		background: var(--bg-page);
		border: 1px solid var(--border-default);
		border-radius: var(--radius-sm, 10px);
		padding: var(--space-2);
		display: flex;
		flex-direction: column;
		gap: var(--space-1);
		transition: border-color 0.15s ease;
	}
	.prompt-box:focus-within {
		border-color: var(--accent-200);
	}
	.chat-input-area textarea {
		width: 100%;
		background: transparent;
		border: none;
		border-radius: 0;
		padding: var(--space-1) var(--space-2);
		color: var(--text-primary);
		outline: none;
		resize: none;
		font-family: inherit;
		line-height: 1.4;
		overflow-y: auto;
	}
	.selection-pill {
		display: inline-flex;
		align-items: center;
		gap: 5px;
		height: 24px;
		padding: 0 8px;
		border-radius: 12px;
		border: 1px solid var(--accent, #6366f1);
		background: color-mix(in srgb, var(--accent, #6366f1) 16%, transparent);
		color: var(--accent, #6366f1);
		font-size: 0.72rem;
		font-weight: 600;
		cursor: pointer;
		white-space: nowrap;
		transition: background 0.12s ease;
	}
	.selection-pill:hover {
		background: color-mix(in srgb, var(--accent, #6366f1) 26%, transparent);
	}
	.selection-pill .sel-x {
		opacity: 0.6;
		font-size: 0.7rem;
	}
	.selection-pill:hover .sel-x {
		opacity: 1;
	}

	.prompt-toolbar {
		display: flex;
		align-items: center;
		gap: 6px;
		/* Pull to the box edges so the divider spans full width, making the
		   controls a visually distinct section below the input. */
		margin: 0 calc(-1 * var(--space-2)) calc(-1 * var(--space-2));
		padding: var(--space-2);
		border-top: 1px solid var(--border-subtle);
	}
	.prompt-spacer {
		flex: 1;
	}
	.mode-pill {
		font-size: 0.72rem;
		font-weight: 600;
		line-height: 1;
		padding: 4px 10px;
		border-radius: 999px;
		border: 1px solid var(--border-default);
		background: transparent;
		color: var(--text-secondary);
		cursor: pointer;
		transition: background 0.15s, color 0.15s, border-color 0.15s, opacity 0.15s;
	}
	.mode-pill.auto {
		color: #1c1c1c;
		background: #e0b341;
		border-color: transparent;
	}
	.mode-pill:hover {
		opacity: 0.88;
	}
	.prompt-icon-btn,
	.send-btn {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		width: 28px;
		height: 28px;
		border-radius: 7px;
		border: none;
		background: transparent;
		color: var(--text-secondary);
		cursor: pointer;
		transition: background 0.15s, color 0.15s, opacity 0.15s;
	}
	.prompt-icon-btn:hover {
		background: var(--bg-hover, rgba(255, 255, 255, 0.07));
		color: var(--text-primary);
	}
	.send-btn {
		color: var(--accent-200, #6ea8fe);
	}
	.send-btn:hover:not(:disabled) {
		background: var(--bg-hover, rgba(255, 255, 255, 0.07));
	}
	.send-btn:disabled {
		color: var(--text-muted);
		opacity: 0.45;
		cursor: default;
	}
	.context-ring svg {
		display: block;
		transform: rotate(-90deg);
	}
	.context-ring .ring-track {
		fill: none;
		stroke: var(--border-default);
		stroke-width: 3;
	}
	.context-ring .ring-value {
		fill: none;
		stroke-width: 3;
		stroke-linecap: round;
		transition: stroke-dashoffset 0.3s ease, stroke 0.3s ease;
	}
	.chat-input-area textarea::-webkit-scrollbar {
		width: 6px;
	}
	.chat-input-area textarea::-webkit-scrollbar-track {
		background: transparent;
	}
	.chat-input-area textarea::-webkit-scrollbar-thumb {
		background: var(--neutral-700, #444);
		border-radius: 3px;
	}
	.chat-input-area textarea::-webkit-scrollbar-thumb:hover {
		background: var(--neutral-500, #666);
	}

	.loading-dots::after {
		content: '...';
		animation: blink 1.5s steps(4, end) infinite;
	}
	@keyframes blink {
		0%,
		20% {
			color: transparent;
		}
		40% {
			color: inherit;
		}
		100% {
			color: inherit;
		}
	}

	.chat-msg-actions {
		display: flex;
		gap: var(--space-1);
		margin-top: 3px;
		justify-content: flex-end;
	}
	.rewind-btn {
		padding: 1px 6px;
		font-size: 13px;
		background: transparent;
		border: 1px solid transparent;
		border-radius: var(--radius-sm);
		color: color-mix(in srgb, var(--text-tertiary) 50%, transparent);
		cursor: pointer;
		transition:
			color 0.15s,
			border-color 0.15s;
		line-height: 1.6;
	}
	.rewind-btn:hover {
		color: var(--text-secondary);
		border-color: var(--border-color);
	}
	.rewind-btn.retry:hover {
		color: var(--accent);
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
	
	.chat-time-taken {
		display: block;
		font-size: 0.75rem;
		color: var(--text-secondary);
		opacity: 0.7;
		margin-top: var(--space-2);
		user-select: none;
	}
	
	.chat-time-taken.live {
		color: var(--accent);
		opacity: 0.9;
	}

	/* Animated "working" dots shown while a turn is running but has produced no
	   text yet (model thinking, or a tool like web search/fetch executing) — so a
	   slow turn reads as alive, not frozen. */
	.chat-working {
		display: inline-flex;
		gap: 4px;
		align-items: center;
		margin-top: var(--space-2);
		margin-right: var(--space-2);
		vertical-align: middle;
	}

	.chat-working span {
		width: 5px;
		height: 5px;
		border-radius: 50%;
		background: var(--accent);
		animation: chat-working-pulse 1.2s ease-in-out infinite;
	}

	.chat-working span:nth-child(2) {
		animation-delay: 0.2s;
	}

	.chat-working span:nth-child(3) {
		animation-delay: 0.4s;
	}

	@keyframes chat-working-pulse {
		0%, 80%, 100% {
			opacity: 0.25;
			transform: scale(0.8);
		}
		40% {
			opacity: 1;
			transform: scale(1);
		}
	}
</style>
