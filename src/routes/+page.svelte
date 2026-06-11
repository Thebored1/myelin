<script lang="ts">
	import { invoke } from '@tauri-apps/api/core';
	import { listen } from '@tauri-apps/api/event';
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { open } from '@tauri-apps/plugin-dialog';
	import type {
		AppSnapshot,
		NoteDocument,
		NoteSummary,
		ProviderStatus,
		SearchResponse
	} from '$lib/types';
	import { onMount } from 'svelte';

	let app = $state<AppSnapshot | null>(null);
	let provider = $state<ProviderStatus | null>(null);
	let query = $state('');
	let isBusy = $state(false);
	let message = $state('');
	let searchResults = $state<SearchResponse | null>(null);
	let pendingCreateCount = 0;
	let createLoopRunning = false;
	let activeMenuId = $state<string | null>(null);
	let deleteDialog: HTMLDialogElement | undefined = $state();
	let noteToDelete = $state<string | null>(null);

	let visibleNotes = $derived.by(() => {
		const base = (
			query && searchResults
				? searchResults.results.map((r) => r.note)
				: (app?.notes ?? [])
		) as NoteSummary[];
		return [...base].sort((a, b) => b.updatedAt.localeCompare(a.updatedAt));
	});

	let regularNotes = $derived(visibleNotes.filter((n) => !n.relativePath.endsWith('.pdf')));
	let pdfNotes = $derived(visibleNotes.filter((n) => n.relativePath.endsWith('.pdf')));

	let commonplaces = $derived.by(() => {
		if (!app?.notes) return [];
		const graph = new Map<string, Set<string>>();
		app.notes.forEach((note) => {
			if (!graph.has(note.id)) graph.set(note.id, new Set());
			note.backlinks.forEach((link) => {
				if (!graph.has(link.sourceId)) graph.set(link.sourceId, new Set());
				graph.get(note.id)!.add(link.sourceId);
				graph.get(link.sourceId)!.add(note.id);
			});
		});
		const visited = new Set<string>();
		const clusters: NoteSummary[][] = [];
		const noteMap = new Map(app.notes.map((n) => [n.id, n]));
		app.notes.forEach((note) => {
			if (!visited.has(note.id)) {
				const cluster: string[] = [];
				const queue = [note.id];
				visited.add(note.id);
				while (queue.length > 0) {
					const curr = queue.shift()!;
					cluster.push(curr);
					graph.get(curr)?.forEach((neighbor) => {
						if (!visited.has(neighbor)) { visited.add(neighbor); queue.push(neighbor); }
					});
				}
				if (cluster.length > 1) {
					const mapped = cluster.map((id) => noteMap.get(id)).filter((n): n is NoteSummary => !!n);
					if (mapped.length > 1) clusters.push(mapped);
				}
			}
		});
		return clusters.sort((a, b) => b.length - a.length);
	});

	async function refreshApp() {
		app = await invoke<AppSnapshot>('get_snapshot');
		provider = await invoke<ProviderStatus>('get_provider_status');
		if (query.trim()) {
			searchResults = await invoke<SearchResponse>('search_notes', { query });
		} else {
			searchResults = null;
		}
	}

	function folderFromRelativePath(relativePath: string) {
		const segments = relativePath.split('/').filter(Boolean);
		return segments.length > 1 ? segments.slice(0, -1).join('/') : 'Root';
	}

	function excerptFromBody(body: string) {
		const flat = body.trim().replace(/\s+/g, ' ');
		return flat.length > 400 ? `${flat.slice(0, 400)}...` : flat;
	}

	function upsertNoteIntoLibrary(note: NoteDocument) {
		if (!app) return;
		const summary: NoteSummary = {
			id: note.id, title: note.title, tags: note.tags,
			folder: folderFromRelativePath(note.relativePath),
			excerpt: excerptFromBody(note.body),
			relativePath: note.relativePath,
			createdAt: note.createdAt, updatedAt: note.updatedAt, backlinks: note.backlinks
		};
		const existingIndex = app.notes.findIndex((e) => e.id === note.id);
		if (existingIndex >= 0) { app.notes[existingIndex] = summary; }
		else { app.notes = [summary, ...app.notes]; }
		if (!app.customNoteOrder.includes(note.id)) app.customNoteOrder = [...app.customNoteOrder, note.id];
		if (!app.libraryFacets.folders.includes(summary.folder)) app.libraryFacets.folders = [...app.libraryFacets.folders, summary.folder].sort();
		const mergedTags = new Set([...app.libraryFacets.tags, ...summary.tags]);
		app.libraryFacets.tags = [...mergedTags].sort();
	}

	async function pickWorkspace() {
		const picked = await open({ directory: true, multiple: false, title: 'Choose your markdown workspace' });
		if (typeof picked === 'string') {
			isBusy = true;
			try {
				app = await invoke<AppSnapshot>('set_workspace', { workspacePath: picked });
				message = 'Workspace connected.';
			} finally { isBusy = false; }
		}
	}

	async function createNote() {
		pendingCreateCount += 1;
		if (createLoopRunning) return;
		createLoopRunning = true;
		isBusy = true;
		try {
			while (pendingCreateCount > 0) {
				pendingCreateCount -= 1;
				const note = await invoke<NoteDocument>('create_note', { title: 'New note' });
				upsertNoteIntoLibrary(note);
			}
			await refreshApp();
		} finally { isBusy = false; createLoopRunning = false; }
	}

	async function runSearch() {
		if (query.trim()) {
			searchResults = await invoke<SearchResponse>('search_notes', { query });
		} else {
			searchResults = null;
		}
	}

	async function rebuild() {
		isBusy = true;
		try {
			app = await invoke<AppSnapshot>('rebuild_index');
			message = 'Index rebuilt.';
		} finally { isBusy = false; }
	}

	function timeAgo(value: string) {
		const diff = Date.now() - new Date(value).getTime();
		const mins = Math.floor(diff / 60000);
		if (mins < 1) return 'now';
		if (mins < 60) return `${mins}m`;
		const hrs = Math.floor(mins / 60);
		if (hrs < 24) return `${hrs}h`;
		const days = Math.floor(hrs / 24);
		if (days < 7) return `${days}d`;
		return `${Math.floor(days / 7)}w`;
	}

	async function openNote(noteId: string) {
		await goto(resolve(`/notes/${encodeURIComponent(noteId)}`));
	}

	function requestDeleteNote(e: MouseEvent, noteId: string) {
		e.stopPropagation();
		activeMenuId = null;
		noteToDelete = noteId;
		deleteDialog?.showModal();
	}

	async function confirmDelete() {
		if (!noteToDelete) return;
		isBusy = true;
		try {
			app = await invoke<AppSnapshot>('delete_note', { noteId: noteToDelete });
		} catch (e) { console.error(e); }
		finally { isBusy = false; deleteDialog?.close(); noteToDelete = null; }
	}

	function workspaceLabel(path: string) {
		const parts = path.replace(/\\/g, '/').split('/');
		return parts[parts.length - 1] || path;
	}

	onMount(() => {
		let unlistenChanged = () => {};
		let unlistenStatus = () => {};
		void (async () => {
			app = await invoke<AppSnapshot>('bootstrap');
			await refreshApp();
			unlistenChanged = await listen('index://changed', () => { message = 'Reindexing…'; });
			unlistenStatus = await listen<string>('index://status', (event) => {
				if (event.payload === 'started') message = 'Indexing…';
				else if (event.payload === 'completed') { message = ''; void refreshApp(); }
			});
		})();
		return () => { unlistenChanged(); unlistenStatus(); };
	});
</script>

<svelte:head><title>myelin</title></svelte:head>
<svelte:window onclick={() => { activeMenuId = null; }} />

<dialog bind:this={deleteDialog} class="confirm-dialog" onclose={() => { noteToDelete = null; }}>
	<div class="dialog-content">
		<h3>Delete note?</h3>
		<p>This cannot be undone.</p>
		<div class="dialog-actions">
			<button class="btn-ghost" onclick={() => deleteDialog?.close()}>Cancel</button>
			<button class="btn-danger" onclick={confirmDelete} disabled={isBusy}>Delete</button>
		</div>
	</div>
</dialog>

<div class="shell">
	<!-- ── Left rail: library ─────────────────────────────── -->
	<aside class="rail">
		<div class="rail-top">
			<span class="wordmark">myelin</span>
			<button class="btn-new" onclick={createNote} disabled={isBusy || !app?.workspacePath} title="New note">
				<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round"><line x1="12" y1="5" x2="12" y2="19"/><line x1="5" y1="12" x2="19" y2="12"/></svg>
				New Note
			</button>
		</div>

		<div class="rail-search">
			<svg class="search-icon" width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><circle cx="11" cy="11" r="8"/><line x1="21" y1="21" x2="16.65" y2="16.65"/></svg>
			<input bind:value={query} oninput={runSearch} placeholder="Search…" class="search-input" />
		</div>

		<div class="rail-list">
			{#if !app?.workspacePath}
				<p class="rail-empty">No workspace selected.</p>
			{:else if regularNotes.length === 0 && pdfNotes.length === 0}
				<p class="rail-empty">No notes yet.</p>
			{:else}
				{#if regularNotes.length > 0}
					<div class="section-label">
						<span>Notes</span>
						<span class="section-count">{regularNotes.length}</span>
					</div>
					{#each regularNotes as note (note.id)}
						<!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
						<div
							class="note-row"
							onclick={() => openNote(note.id)}
							oncontextmenu={(e) => { e.preventDefault(); activeMenuId = activeMenuId === note.id ? null : note.id; }}
						>
							<svg class="row-icon" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><polyline points="14 2 14 8 20 8"/></svg>
							<span class="row-title">{note.title}</span>
							<span class="row-time">{timeAgo(note.updatedAt)}</span>
							<div class="row-menu-wrap">
								<button class="row-menu-btn" onclick={(e) => { e.stopPropagation(); activeMenuId = activeMenuId === note.id ? null : note.id; }} aria-label="Options">
									<svg width="12" height="12" viewBox="0 0 24 24" fill="currentColor"><circle cx="12" cy="5" r="1.5"/><circle cx="12" cy="12" r="1.5"/><circle cx="12" cy="19" r="1.5"/></svg>
								</button>
								{#if activeMenuId === note.id}
									<div class="row-dropdown">
										<button class="row-delete" onclick={(e) => requestDeleteNote(e, note.id)}>Delete</button>
									</div>
								{/if}
							</div>
						</div>
					{/each}
				{/if}

				{#if pdfNotes.length > 0}
					<div class="section-label" style="margin-top: var(--space-4);">
						<span>PDFs</span>
						<span class="section-count">{pdfNotes.length}</span>
					</div>
					{#each pdfNotes as note (note.id)}
						<!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
						<div class="note-row" onclick={() => openNote(note.id)}>
							<svg class="row-icon pdf-icon" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><polyline points="14 2 14 8 20 8"/><line x1="9" y1="13" x2="15" y2="13"/><line x1="9" y1="17" x2="11" y2="17"/></svg>
							<span class="row-title">{note.title}</span>
							<span class="row-time">{timeAgo(note.updatedAt)}</span>
							<div class="row-menu-wrap">
								<button class="row-menu-btn" onclick={(e) => { e.stopPropagation(); activeMenuId = activeMenuId === note.id ? null : note.id; }} aria-label="Options">
									<svg width="12" height="12" viewBox="0 0 24 24" fill="currentColor"><circle cx="12" cy="5" r="1.5"/><circle cx="12" cy="12" r="1.5"/><circle cx="12" cy="19" r="1.5"/></svg>
								</button>
								{#if activeMenuId === note.id}
									<div class="row-dropdown">
										<button class="row-delete" onclick={(e) => requestDeleteNote(e, note.id)}>Delete</button>
									</div>
								{/if}
							</div>
						</div>
					{/each}
				{/if}
			{/if}
		</div>

		<div class="rail-footer">
			<button class="footer-change-btn" onclick={pickWorkspace} disabled={isBusy} title={app?.workspacePath ?? 'Choose workspace'}>
				<svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"/></svg>
				{#if app?.workspacePath}
					{workspaceLabel(app.workspacePath)}
				{:else}
					Connect workspace
				{/if}
			</button>
			{#if app?.workspacePath}
				<span class="footer-dot" class:dot-ok={provider?.activeProvider} title={provider?.activeProvider ?? 'No provider'}></span>
			{/if}
		</div>
	</aside>

	<!-- ── Right: workspace panel ─────────────────────────── -->
	<main class="workspace">
		{#if !app?.workspacePath}
			<div class="landing">
				<p class="eyebrow">Cross-platform local notes</p>
				<h1>myelin</h1>
				<p class="landing-copy">A local-first markdown workspace. Connect a folder to get started.</p>
				<button class="btn-primary" onclick={pickWorkspace} disabled={isBusy}>Choose workspace</button>
			</div>
		{:else}
			<div class="workspace-inner">
				{#if message}<p class="ws-status-line">{message}</p>{/if}
				{#if commonplaces.length > 0}
					<div class="commonplace-section">
						<div class="section-label flat">
							<span>Commonplace</span>
							<span class="section-count">{commonplaces.length}</span>
						</div>
						<div class="clusters-grid">
							{#each commonplaces as cluster, i}
								<div class="cluster-card">
									<div class="cluster-head">
										<span class="cluster-label">Cluster {i + 1}</span>
										<span class="cluster-count">{cluster.length} notes</span>
									</div>
									<div class="cluster-pills">
										{#each cluster as note}
											<button class="pill" onclick={() => openNote(note.id)}>{note.title}</button>
										{/each}
									</div>
								</div>
							{/each}
						</div>
					</div>
				{:else}
					<div class="workspace-empty">
						<p>Select a note to open it, or create a new one.</p>
					</div>
				{/if}
			</div>
		{/if}
	</main>
</div>

<style>
	/* ── Layout ── */
	.shell {
		display: grid;
		grid-template-columns: 300px 1fr;
		height: calc(100vh - 32px);
		overflow: hidden;
		animation: fade-in 0.2s ease-out;
	}

	/* ── Rail ── */
	.rail {
		display: flex;
		flex-direction: column;
		border-right: 1px solid var(--border-default);
		background: var(--bg-panel);
		overflow: hidden;
		font-family: var(--font-mono);
	}

	.rail-top {
		display: flex;
		align-items: center;
		justify-content: space-between;
		padding: var(--space-5) var(--space-5) var(--space-4);
		gap: var(--space-3);
		flex-shrink: 0;
	}

	.wordmark {
		font-size: 1.05rem;
		font-weight: 700;
		letter-spacing: -0.03em;
		color: var(--text-hero);
	}

	.btn-new {
		display: flex;
		align-items: center;
		gap: var(--space-2);
		padding: 6px 13px;
		font-size: 0.8rem;
		font-family: var(--font-mono);
		background: var(--accent-200);
		color: #fff;
		border: none;
		border-radius: var(--radius-sm);
		cursor: pointer;
		transition: background 0.15s;
		white-space: nowrap;
	}
	.btn-new:hover:not(:disabled) { background: var(--accent-100); }
	.btn-new:disabled { opacity: 0.5; cursor: not-allowed; }

	.rail-search {
		position: relative;
		padding: 0 var(--space-4) var(--space-4);
		flex-shrink: 0;
	}
	.search-icon {
		position: absolute;
		left: calc(var(--space-4) + 11px);
		top: 50%;
		transform: translateY(-60%);
		color: var(--text-secondary);
		pointer-events: none;
	}
	.search-input {
		width: 100%;
		box-sizing: border-box;
		padding: 8px 12px 8px 32px;
		font-size: 0.82rem;
		font-family: var(--font-mono);
		background: var(--bg-page);
		border: 1px solid var(--border-default);
		border-radius: var(--radius-sm);
		color: var(--text-primary);
		outline: none;
	}
	.search-input::placeholder { color: var(--neutral-600); }
	.search-input:focus { border-color: var(--neutral-600); }

	.rail-list {
		flex: 1;
		overflow-y: auto;
		padding: 0 var(--space-3) var(--space-4);
		scrollbar-width: none;
	}
	.rail-list::-webkit-scrollbar { display: none; }

	.rail-empty {
		font-size: 0.82rem;
		color: var(--text-secondary);
		padding: var(--space-4) var(--space-3);
		margin: 0;
	}

	.section-label {
		display: flex;
		justify-content: space-between;
		align-items: center;
		padding: var(--space-3) var(--space-3);
		font-size: 0.68rem;
		font-weight: 700;
		letter-spacing: 0.08em;
		text-transform: uppercase;
		color: var(--neutral-500);
		user-select: none;
	}
	.section-label.flat { padding: var(--space-2) 0; }
	.section-count {
		font-size: 0.68rem;
		color: var(--neutral-700);
		font-weight: 400;
	}

	.note-row {
		display: flex;
		align-items: center;
		gap: var(--space-3);
		padding: 8px var(--space-3);
		border-radius: var(--radius-xs);
		cursor: pointer;
		position: relative;
		border: 1px solid transparent;
		transition: background 0.1s, border-color 0.1s;
	}
	.note-row:hover {
		background: rgba(255,255,255,0.04);
		border-color: var(--border-default);
	}
	.note-row:hover .row-menu-btn { opacity: 1; }

	.row-icon {
		flex-shrink: 0;
		color: var(--neutral-600);
	}
	.row-icon.pdf-icon { color: var(--accent-300); }

	.row-title {
		flex: 1;
		font-size: 0.875rem;
		color: var(--text-primary);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
		line-height: 1.3;
	}

	.row-time {
		flex-shrink: 0;
		font-size: 0.72rem;
		color: var(--neutral-600);
		font-family: var(--font-mono);
	}

	.row-menu-wrap {
		position: relative;
		flex-shrink: 0;
	}
	.row-menu-btn {
		background: transparent;
		border: none;
		padding: 2px 3px;
		color: var(--neutral-500);
		cursor: pointer;
		border-radius: var(--radius-xs);
		display: flex;
		align-items: center;
		opacity: 0;
		transition: opacity 0.1s, background 0.1s;
	}
	.row-menu-btn:hover { background: rgba(255,255,255,0.08); color: var(--text-primary); }

	.row-dropdown {
		position: absolute;
		right: 0;
		top: 100%;
		z-index: 20;
		background: var(--bg-panel);
		border: 1px solid var(--border-default);
		border-radius: var(--radius-sm);
		padding: var(--space-1);
		min-width: 100px;
		box-shadow: 0 4px 12px rgba(0,0,0,0.3);
	}
	.row-delete {
		width: 100%;
		text-align: left;
		padding: var(--space-2) var(--space-3);
		font-size: 0.82rem;
		font-family: var(--font-mono);
		background: transparent;
		color: #e05555;
		border: none;
		border-radius: var(--radius-xs);
		cursor: pointer;
	}
	.row-delete:hover { background: rgba(224,85,85,0.1); }

	.rail-footer {
		flex-shrink: 0;
		padding: var(--space-3) var(--space-4);
		border-top: 1px solid var(--border-default);
		display: flex;
		align-items: center;
		gap: var(--space-2);
	}
	.footer-change-btn {
		flex: 1;
		display: flex;
		align-items: center;
		gap: 6px;
		padding: 8px var(--space-3);
		font-size: 0.8rem;
		font-family: var(--font-mono);
		background: transparent;
		border: 1px solid transparent;
		border-radius: var(--radius-xs);
		color: var(--neutral-500);
		cursor: pointer;
		text-align: left;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
		transition: color 0.1s, border-color 0.1s, background 0.1s;
	}
	.footer-change-btn:hover:not(:disabled) {
		color: var(--text-primary);
		border-color: var(--border-default);
		background: rgba(255,255,255,0.04);
	}
	.footer-change-btn:disabled { opacity: 0.4; cursor: not-allowed; }
	.footer-dot {
		width: 6px;
		height: 6px;
		border-radius: 50%;
		background: var(--neutral-700);
		flex-shrink: 0;
	}
	.footer-dot.dot-ok { background: #4caf50; }

	/* ── Workspace panel ── */
	.workspace {
		overflow-y: auto;
		background: var(--bg-page);
	}

	.landing {
		display: flex;
		flex-direction: column;
		align-items: flex-start;
		justify-content: center;
		height: 100%;
		padding: 4rem 5rem;
		gap: var(--space-6);
		font-family: var(--font-mono);
	}
	.eyebrow {
		font-size: 0.75rem;
		font-weight: 700;
		text-transform: uppercase;
		letter-spacing: 0.1em;
		color: var(--text-secondary);
		margin: 0;
	}
	h1 {
		font-size: 4rem;
		font-weight: 800;
		letter-spacing: -0.04em;
		color: var(--text-hero);
		margin: 0;
		line-height: 1;
	}
	.landing-copy {
		color: var(--text-secondary);
		font-size: 1rem;
		line-height: 1.6;
		max-width: 28rem;
		margin: 0;
	}
	.btn-primary {
		padding: 12px 24px;
		font-family: var(--font-mono);
		font-size: 0.925rem;
		background: var(--accent-200);
		color: #fff;
		border: none;
		border-radius: var(--radius-sm);
		cursor: pointer;
		transition: background 0.15s;
	}
	.btn-primary:hover:not(:disabled) { background: var(--accent-100); }
	.btn-primary:disabled { opacity: 0.5; }

	.workspace-inner {
		padding: var(--space-10) var(--space-10);
		display: flex;
		flex-direction: column;
		gap: var(--space-6);
		font-family: var(--font-mono);
		max-width: 800px;
		height: 100%;
	}
	.ws-status-line {
		margin: 0;
		font-size: 0.7rem;
		color: var(--neutral-500);
	}
	.workspace-empty {
		flex: 1;
		display: flex;
		align-items: center;
		justify-content: center;
	}
	.workspace-empty p {
		font-size: 0.9rem;
		color: var(--neutral-600);
		margin: 0;
	}

	/* Commonplace */
	.commonplace-section {
		display: flex;
		flex-direction: column;
		gap: var(--space-4);
	}
	.clusters-grid {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(200px, 1fr));
		gap: var(--space-3);
	}
	.cluster-card {
		background: var(--bg-panel);
		border: 1px solid var(--border-default);
		border-radius: var(--radius-sm);
		padding: var(--space-4);
		display: flex;
		flex-direction: column;
		gap: var(--space-3);
	}
	.cluster-head {
		display: flex;
		justify-content: space-between;
		align-items: center;
	}
	.cluster-label { font-size: 0.72rem; color: var(--text-primary); font-weight: 600; }
	.cluster-count { font-size: 0.65rem; color: var(--neutral-600); }
	.cluster-pills {
		display: flex;
		flex-wrap: wrap;
		gap: var(--space-2);
	}
	.pill {
		font-size: 0.68rem;
		font-family: var(--font-mono);
		padding: 3px 8px;
		background: transparent;
		border: 1px solid var(--border-default);
		border-radius: var(--radius-xs);
		color: var(--text-secondary);
		cursor: pointer;
		transition: border-color 0.1s, color 0.1s;
		text-align: left;
		max-width: 100%;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}
	.pill:hover { border-color: var(--accent-200); color: var(--text-primary); }

	/* Dialog */
	.confirm-dialog {
		padding: 0 !important;
		border: 1px solid var(--border-default) !important;
		border-radius: var(--radius-sm) !important;
		background: var(--bg-panel) !important;
		color: var(--text-primary) !important;
		max-width: 20rem !important;
		width: 100% !important;
		box-shadow: 0 8px 32px rgba(0,0,0,0.4) !important;
	}
	.confirm-dialog::backdrop {
		background: rgba(0,0,0,0.5) !important;
		backdrop-filter: blur(4px) !important;
	}
	.dialog-content {
		padding: var(--space-6);
		display: flex;
		flex-direction: column;
		gap: var(--space-3);
		font-family: var(--font-mono);
	}
	.dialog-content h3 { margin: 0; font-size: 0.95rem; color: var(--text-hero); }
	.dialog-content p { margin: 0; font-size: 0.75rem; color: var(--text-secondary); }
	.dialog-actions { display: flex; justify-content: flex-end; gap: var(--space-2); margin-top: var(--space-2); }
	.btn-ghost {
		padding: 6px 14px;
		font-size: 0.75rem;
		font-family: var(--font-mono);
		background: transparent;
		border: 1px solid var(--border-default);
		border-radius: var(--radius-sm);
		color: var(--text-secondary);
		cursor: pointer;
	}
	.btn-ghost:hover { color: var(--text-primary); }
	.btn-danger {
		padding: 6px 14px;
		font-size: 0.75rem;
		font-family: var(--font-mono);
		background: transparent;
		border: 1px solid #e05555;
		border-radius: var(--radius-sm);
		color: #e05555;
		cursor: pointer;
	}
	.btn-danger:hover:not(:disabled) { background: rgba(224,85,85,0.1); }
	.btn-danger:disabled { opacity: 0.4; }

	@keyframes fade-in {
		from { opacity: 0; transform: translateY(4px); }
		to { opacity: 1; transform: translateY(0); }
	}

	@media (max-width: 700px) {
		.shell { grid-template-columns: 1fr; }
		.workspace { display: none; }
	}
</style>
