<script lang="ts">
	import { invoke } from '@tauri-apps/api/core';
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import type { NoteDocument, SearchResponse, NoteSummary } from '$lib/types';
	import { onMount, onDestroy } from 'svelte';
	import { sidebarOpen, showSidebarToggle } from '$lib/stores';
	import Vditor from 'vditor';
	import 'vditor/dist/index.css';
	import 'mathlive';
	import 'mathlive/fonts.css';

	let note = $state<NoteDocument | null>(null);
	let draftBody = $state('');
	let draftTitle = $state('');
	let draftTags = $state('');
	let isBusy = $state(false);
	let message = $state('');
	
	let relatedNotes = $state<NoteSummary[]>([]);
	let vditorContainer: HTMLElement | undefined = $state();
	let vditorInstance: Vditor | null = null;
	
	let mathDialog: HTMLDialogElement | undefined = $state();
	let mathValue = $state('');
	
	let toolbarExpanded = $state(false);
	let toolbarNeedsToggle = $state(false);
	let toolbarResizeObserver: ResizeObserver | null = null;
	
	let saveStatus = $state<'saved' | 'saving' | 'unsaved'>('saved');
	let saveTimer: ReturnType<typeof setTimeout> | null = null;

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

	async function loadCurrentNote() {
		const noteId = page.params.id;
		note = await invoke<NoteDocument>('load_note', { noteId });
		draftTitle = note.title;
		draftBody = note.body;
		draftTags = note.tags.join(', ');
		message = '';
		void fetchRelatedNotes();
		
		if (vditorContainer) {
			vditorInstance = new Vditor(vditorContainer, {
				value: draftBody,
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
					"|",
					"upload", "record", "table", "|", "undo", "redo", "|", "fullscreen", "edit-mode",
					{
						name: "more",
						toolbar: [
							"both", "code-theme", "content-theme", "export", "outline", "preview", "devtools", "info", "help"
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
							updateToolbarOverflow();
						}
					}
				},
				keydown: (e: KeyboardEvent) => {
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
		}
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

	async function saveNote() {
		if (!note) return;
		isBusy = true;
		saveStatus = 'saving';
		try {
			note = await invoke<NoteDocument>('save_note', {
				noteId: note.id,
				title: draftTitle,
				tags: draftTags
					.split(',')
					.map((tag) => tag.trim())
					.filter(Boolean),
				body: draftBody
			});
			saveStatus = 'saved';
			void fetchRelatedNotes();
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
			window.location.href = resolve(`/notes/${encodeURIComponent(duplicated.id)}`);
		} finally {
			isBusy = false;
		}
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

	onMount(() => {
		$showSidebarToggle = true;
		void loadCurrentNote();
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

		return () => {
			mql.removeEventListener('change', handleMediaChange);
			$showSidebarToggle = false;
		};
	});

	onDestroy(() => {
		if (toolbarResizeObserver) toolbarResizeObserver.disconnect();
		if (vditorInstance) vditorInstance.destroy();
	});
</script>

<svelte:head>
	<title>{note ? `${note.title} • myelin` : 'myelin'}</title>
</svelte:head>

<div class="editor-shell">
	<header class="editor-header">
		<div class="header-copy">
			<button class="back-link" onclick={() => goto(resolve('/'))} aria-label="Back to library" title="Back to library">
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

	<div class="main-layout">
		<!-- Main Content Area -->
		<section class="main-pane">
			<div class="content-area" style="position: relative;">
				<div bind:this={vditorContainer} class="vditor-wrapper" class:toolbar-expanded={toolbarExpanded}></div>
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
		<aside class="sidebar" class:open={$sidebarOpen}>
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
						{#each relatedNotes as rel (rel.id)}
							<li><a href="/notes/{encodeURIComponent(rel.id)}">{rel.title}</a></li>
						{/each}
					</ul>
				{:else}
					<p class="empty-state">No related notes found.</p>
				{/if}
			</div>

			<div class="sidebar-section">
				<h3>Backlinks</h3>
				<p class="empty-state">No backlinks yet.</p>
			</div>
		</aside>
	</div>
</div>

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

<style>
	.editor-shell {
		min-height: 100vh;
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
	.header-actions button,
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

	.sidebar-toggle-btn {
		display: flex;
		align-items: center;
		justify-content: center;
		padding: 0.5rem;
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
		align-items: center;
	}

	.content-area {
		width: 100%;
		flex: 1;
		display: flex;
		flex-direction: column;
	}

	.vditor-wrapper {
		flex: 1;
		min-height: 0;
		border: none !important;
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
		align-items: center !important;
		background: var(--bg-page) !important;
		flex: 1 !important;
		min-height: 0 !important;
		overflow-y: auto !important;
	}

	:global(.vditor-ir) {
		width: 100% !important;
		max-width: 210mm !important;
		margin: 0 auto !important;
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

	/* Force Vditor toolbar tooltips to drop downwards to avoid WebKitGTK header overlap bugs */
	:global(.vditor-tooltipped__n::after) {
		top: 100% !important;
		bottom: auto !important;
		margin-top: 5px !important;
		margin-bottom: 0 !important;
	}

	:global(.vditor-tooltipped__n::before) {
		display: none !important;
	}

	/* Sidebar (Mobile / Overlay mode by default) */
	.sidebar {
		position: absolute;
		top: 0;
		right: 0;
		bottom: 0;
		width: 20rem;
		max-width: 85vw;
		background: var(--bg-panel);
		padding: var(--space-6);
		display: flex;
		flex-direction: column;
		gap: var(--space-6);
		overflow-y: auto;
		z-index: 100;
		transform: translateX(100%);
		transition: transform 0.3s cubic-bezier(0.16, 1, 0.3, 1), margin-right 0.3s cubic-bezier(0.16, 1, 0.3, 1);
		border-left: 1px solid var(--border-default);
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
			margin-right: -20rem;
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
		background: var(--bg-page);
		color: var(--text-primary);
		max-width: 40rem;
		width: 100%;
		backdrop-filter: blur(var(--blur-md));
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
</style>
