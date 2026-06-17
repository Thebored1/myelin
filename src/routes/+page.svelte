<script lang="ts">
	import { invoke } from '@tauri-apps/api/core';
	import { listen } from '@tauri-apps/api/event';
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { open } from '@tauri-apps/plugin-dialog';
	import { sidebarOpen } from '$lib/stores';
	import type {
		AppSnapshot,
		NoteDocument,
		NoteSummary,
		ProviderStatus,
		SearchResponse
	} from '$lib/types';
	import { onMount } from 'svelte';

	let app = $state<AppSnapshot | null>(null);
	// True once the initial snapshot has loaded — prevents the "no workspace"
	// welcome screen from flashing before we know if a workspace is connected.
	let ready = $state(false);
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

	let dashTasks = $state<{id: number, text: string, done: boolean}[]>([]);
	let currentWorkspaceForTasks = $state<string | null>(null);
	let pinnedNoteIds = $state<string[]>([]);
	let showTimeline = $state(true);

	$effect(() => {
		if (app?.workspacePath && app.workspacePath !== currentWorkspaceForTasks) {
			currentWorkspaceForTasks = app.workspacePath;
			const stored = localStorage.getItem(`tasks_${app.workspacePath}`);
			if (stored) {
				try {
					dashTasks = JSON.parse(stored);
				} catch { dashTasks = []; }
			} else {
				dashTasks = [];
			}
			const storedPinned = localStorage.getItem(`pinned_${app.workspacePath}`);
			if (storedPinned) {
				try {
					pinnedNoteIds = JSON.parse(storedPinned);
				} catch { pinnedNoteIds = []; }
			} else {
				pinnedNoteIds = [];
			}
			const storedTimeline = localStorage.getItem(`timeline_${app.workspacePath}`);
			if (storedTimeline !== null) {
				showTimeline = storedTimeline === 'true';
			} else {
				showTimeline = true;
			}
		}
	});

	function getNoteBadge(note: NoteSummary) {
		const rel = note.relativePath.toLowerCase();
		if (rel.endsWith('.pdf')) return 'pdf';
		if (rel.endsWith('.epub')) return 'epub';
		if (rel.endsWith('.tex')) return 'tex';
		if (rel.endsWith('.ipynb')) return 'jupyter';
		if (note.sourcePdf) return 'note + pdf';
		return 'note';
	}

	$effect(() => {
		const toSave = JSON.stringify(dashTasks);
		if (currentWorkspaceForTasks) {
			localStorage.setItem(`tasks_${currentWorkspaceForTasks}`, toSave);
			localStorage.setItem(`pinned_${currentWorkspaceForTasks}`, JSON.stringify(pinnedNoteIds));
			localStorage.setItem(`timeline_${currentWorkspaceForTasks}`, showTimeline.toString());
		}
	});

	let newTaskText = $state('');

	let isClusterDialogOpen = $state(false);
	let selectedCluster = $state<NoteSummary[]>([]);
	

	function togglePin(id: string) {
		if (pinnedNoteIds.includes(id)) {
			pinnedNoteIds = pinnedNoteIds.filter(pid => pid !== id);
		} else {
			pinnedNoteIds = [...pinnedNoteIds, id];
		}
		activeMenuId = null;
	}

	function openCluster(cluster: NoteSummary[]) {
		selectedCluster = cluster;
		isClusterDialogOpen = true;
	}
	function closeClusterDialog() {
		isClusterDialogOpen = false;
	}

	function addTask() {
		if (newTaskText.trim()) {
			dashTasks = [...dashTasks, { id: Date.now(), text: newTaskText.trim(), done: false }];
			newTaskText = '';
		}
	}
	function removeTask(id: number) {
		dashTasks = dashTasks.filter((t) => t.id !== id);
	}

	let visibleNotes = $derived.by(() => {
		const base = (
			query && searchResults
				? searchResults.results.map((r) => r.note)
				: (app?.notes ?? [])
		) as NoteSummary[];
		return [...base].sort((a, b) => b.createdAt.localeCompare(a.createdAt));
	});



	// Rail details — always based on the full library, unaffected by the rail search
	let allNotesSorted = $derived.by(() =>
		[...(app?.notes ?? [])].sort((a, b) => b.createdAt.localeCompare(a.createdAt))
	);
	let dashNotes = $derived(allNotesSorted);
	let dashPdfs = $derived(allNotesSorted.filter((n) => n.relativePath.toLowerCase().endsWith('.pdf')));

	let timelineNotes = $derived(dashNotes.filter(n => new Date(n.createdAt).toDateString() === new Date().toDateString()));
	let pinnedNotes = $derived(
		dashNotes.filter(n => pinnedNoteIds.includes(n.id))
	);

	let totalLinks = $derived((app?.notes ?? []).reduce((sum, n) => sum + n.backlinks.length, 0));
	let tagCounts = $derived.by(() => {
		const counts = new Map<string, number>();
		app?.notes.forEach((n) =>
			n.tags.forEach((t) => {
				const tag = t.trim();
				if (tag) counts.set(tag, (counts.get(tag) ?? 0) + 1);
			})
		);
		return [...counts.entries()].sort((a, b) => b[1] - a[1]);
	});

	let folderStats = $derived.by(() => {
		const stats = new Map<string, { notes: number; pdfs: number }>();
		app?.notes.forEach((n) => {
			const entry = stats.get(n.folder) ?? { notes: 0, pdfs: 0 };
			if (n.relativePath.toLowerCase().endsWith('.pdf')) entry.pdfs += 1;
			else entry.notes += 1;
			stats.set(n.folder, entry);
		});
		return [...stats.entries()].sort(
			(a, b) => b[1].notes + b[1].pdfs - (a[1].notes + a[1].pdfs)
		);
	});

	function searchTag(tag: string) {
		query = tag;
		void runSearch();
	}

	function scrollToSection(id: string) {
		document.getElementById(id)?.scrollIntoView({ behavior: 'smooth', block: 'start' });
	}

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

	async function createNote(extension: string = 'md') {
		pendingCreateCount += 1;
		if (createLoopRunning) return;
		createLoopRunning = true;
		isBusy = true;
		try {
			while (pendingCreateCount > 0) {
				pendingCreateCount -= 1;
				const title = extension === 'md' ? 'New note' :
				              extension === 'tex' ? 'New LaTeX Document' :
				              extension === 'ipynb' ? 'New Jupyter Notebook' :
				              extension === 'epub' ? 'New EPUB Book' : 'New note';
				const note = await invoke<NoteDocument>('create_note', { title, extension });
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

	function agoLabel(value: string) {
		const t = timeAgo(value);
		return t === 'now' ? 'just now' : `${t} ago`;
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
			try {
				app = await invoke<AppSnapshot>('bootstrap');
				await refreshApp();
			} finally {
				ready = true;
			}
			unlistenChanged = await listen('index://changed', () => { message = 'Reindexing…'; });
			unlistenStatus = await listen<string>('index://status', (event) => {
				if (event.payload === 'started') message = 'Indexing…';
				else if (event.payload === 'completed') { message = ''; void refreshApp(); }
			});
		})();
		return () => { unlistenChanged(); unlistenStatus(); };
	});

	let globalSearchDialog: HTMLDialogElement | undefined = $state();
	let globalSearchQuery = $state('');
	let globalSelectedIndex = $state(0);

	let filteredGlobalNotes = $derived.by(() => {
		if (!app?.notes) return [];
		const q = globalSearchQuery.trim().toLowerCase();
		if (!q) return app.notes;
		return app.notes.filter(n => 
			n.title.toLowerCase().includes(q) || 
			n.relativePath.toLowerCase().includes(q) || 
			n.tags.some(t => t.toLowerCase().includes(q))
		);
	});

	function handleGlobalSearchKeydown(e: KeyboardEvent) {
		if (e.key === 'ArrowDown') {
			e.preventDefault();
			if (globalSelectedIndex < filteredGlobalNotes.length - 1) globalSelectedIndex++;
		} else if (e.key === 'ArrowUp') {
			e.preventDefault();
			if (globalSelectedIndex > 0) globalSelectedIndex--;
		} else if (e.key === 'Enter') {
			e.preventDefault();
			if (filteredGlobalNotes.length > 0 && filteredGlobalNotes[globalSelectedIndex]) {
				openNoteFromSearch(filteredGlobalNotes[globalSelectedIndex]);
			}
		} else if (e.key === 'Escape') {
			e.preventDefault();
			globalSearchDialog?.close();
		}
	}

	function openNoteFromSearch(note: NoteSummary) {
		globalSearchDialog?.close();
		openNote(note.id);
	}

	function autofocus(node: HTMLElement) {
		setTimeout(() => node.focus(), 10);
		return { destroy() {} };
	}
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

<dialog bind:this={globalSearchDialog} class="link-dialog" onkeydown={handleGlobalSearchKeydown} onclose={() => { globalSearchQuery = ''; globalSelectedIndex = 0; }}>
	<div class="dialog-content">
		<h3>Link to Note</h3>
		<p style="font-size: 0.875rem; color: var(--text-secondary); margin-bottom: var(--space-4);">Search and select a note from your library.</p>
		
		<input class="link-search-input" bind:value={globalSearchQuery} oninput={() => globalSelectedIndex = 0} use:autofocus placeholder="Search notes..." />
		
		<div class="link-results-container">
			{#if filteredGlobalNotes.length > 0}
				<ul class="link-results-list">
					{#each filteredGlobalNotes as res, i (res.id + '_' + i)}
						<li>
							<button class="link-result-btn" class:selected={i === globalSelectedIndex} onclick={() => openNoteFromSearch(res)}>
								<strong>{res.title}</strong>
								<span class="folder-badge">{folderFromRelativePath(res.relativePath)}</span>
							</button>
						</li>
					{/each}
				</ul>
			{:else if globalSearchQuery.trim()}
				<p class="empty-state">No notes found matching your search.</p>
			{/if}
		</div>
		
		<div class="dialog-actions">
			<button class="btn-ghost" onclick={() => globalSearchDialog?.close()}>Cancel</button>
		</div>
	</div>
</dialog>

<div class="shell" class:rail-collapsed={!$sidebarOpen}>
	<!-- ── Left rail: library ─────────────────────────────── -->
	<aside class="rail">
		<div class="rail-top">
			<span class="wordmark">myelin</span>
		</div>

		<div class="rail-list">
			{#if !ready}
				<!-- Loading: no empty-state flash. -->
			{:else if !app?.workspacePath}
				<p class="rail-empty">No workspace selected.</p>
			{:else if query.trim()}
				{#if visibleNotes.length === 0}
					<p class="rail-empty">No results for “{query}”.</p>
				{/if}
				{#if visibleNotes.length > 0}
					<div class="section-label">
						<span>All Notes</span>
						<span class="section-count">{visibleNotes.length}</span>
					</div>
					{#each visibleNotes as note (note.id)}
						<!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
						<div
							class="note-row"
							onclick={() => openNote(note.id)}
							oncontextmenu={(e) => { e.preventDefault(); activeMenuId = activeMenuId === note.id ? null : note.id; }}
						>
							<svg class="row-icon {note.relativePath.toLowerCase().endsWith('.pdf') ? 'pdf-icon' : ''}" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><polyline points="14 2 14 8 20 8"/>{#if note.relativePath.toLowerCase().endsWith('.pdf')}<line x1="9" y1="13" x2="15" y2="13"/><line x1="9" y1="17" x2="11" y2="17"/>{/if}</svg>
							<span class="row-title">{note.title}</span>
							<div class="row-badge">
								{getNoteBadge(note)}
							</div>
							<span class="row-time">{timeAgo(note.createdAt)}</span>
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
			{:else}
				<!-- Details panel — the page handles browsing, the rail shows workspace vitals -->
				<div class="section-label">
					<span>Overview</span>
				</div>
				<div class="ov-group">
					<div class="ov-row"><span class="ov-key">notes</span><span class="ov-val">{dashNotes.length}</span></div>
					<div class="ov-row"><span class="ov-key">pdfs</span><span class="ov-val">{dashPdfs.length}</span></div>
					<div class="ov-row"><span class="ov-key">tags</span><span class="ov-val">{tagCounts.length}</span></div>
					<div class="ov-row"><span class="ov-key">links</span><span class="ov-val">{totalLinks}</span></div>
					<div class="ov-row"><span class="ov-key">clusters</span><span class="ov-val">{commonplaces.length}</span></div>
				</div>

				<div class="section-label" style="margin-top: var(--space-4);">
					<span>Folders</span>
					<span class="section-count">{folderStats.length}</span>
				</div>
				{#if folderStats.length > 0}
					<div class="ov-group">
						{#each folderStats as [folder, s] (folder)}
							<div class="ov-row">
								<span class="ov-key ov-ellipsis" title={folder}>
									<svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"/></svg>
									{folder.toLowerCase()}
								</span>
								<span class="ov-val">{s.notes + s.pdfs}</span>
							</div>
						{/each}
					</div>
				{:else}
					<p class="rail-empty">Nothing here yet.</p>
				{/if}

				<div class="section-label" style="margin-top: var(--space-4);">
					<span>Tags</span>
					<span class="section-count">{tagCounts.length}</span>
				</div>
				{#if tagCounts.length > 0}
					<div class="ov-group">
						{#each tagCounts.slice(0, 12) as [tag, count] (tag)}
							<button class="ov-row ov-clickable" onclick={() => searchTag(tag)} title="Search “{tag}”">
								<span class="ov-key ov-ellipsis">#{tag}</span>
								<span class="ov-val">{count}</span>
							</button>
						{/each}
					</div>
				{:else}
					<p class="rail-empty">No tags yet.</p>
				{/if}

				<div class="section-label" style="margin-top: var(--space-4);">
					<span>System</span>
				</div>
				<div class="ov-group">
					<div class="ov-row"><span class="ov-key">index</span><span class="ov-val">{app?.indexState.backend ?? '—'}</span></div>
					<div class="ov-row">
						<span class="ov-key">indexed</span>
						<span class="ov-val">{app?.indexState.lastIndexedAt ? agoLabel(app.indexState.lastIndexedAt) : '—'}</span>
					</div>
					<div class="ov-row">
						<span class="ov-key">provider</span>
						<span class="ov-val" class:ov-ok={provider?.healthy}>{provider?.activeProvider || 'none'}</span>
					</div>
				</div>
			{/if}
		</div>

		<div class="rail-footer">
			<button class="footer-change-btn" onclick={pickWorkspace} disabled={isBusy} title={app?.workspacePath ?? 'Choose workspace'}>
				<svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"/></svg>
				{#if !ready}
					&nbsp;
				{:else if app?.workspacePath}
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
		{#if !ready}
			<!-- Initial load: render nothing to avoid flashing the welcome screen. -->
		{:else if !app?.workspacePath}
			<div class="landing">
				<p class="eyebrow">Cross-platform local notes</p>
				<h1>myelin</h1>
				<p class="landing-copy">A local-first markdown workspace. Connect a folder to get started.</p>
				<button class="btn-primary" onclick={pickWorkspace} disabled={isBusy}>Choose workspace</button>
			</div>
		{:else}
			<div class="dashboard-container">
				<!-- Header -->
				<header class="dashboard-header">
					<h2>{app?.workspacePath ? workspaceLabel(app.workspacePath) : 'workspace'}</h2>
					<div style="display: flex; gap: var(--space-2); align-items: center;">
						<button class="header-icon-btn" onclick={() => showTimeline = !showTimeline} title="Toggle Timeline" class:active={showTimeline}>
							<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="10"/><polyline points="12 6 12 12 16 14"/></svg>
						</button>
						<button class="header-search-btn" onclick={() => globalSearchDialog?.showModal()}>
							<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><circle cx="11" cy="11" r="8"/><line x1="21" y1="21" x2="16.65" y2="16.65"/></svg>
							Search notes...
						</button>
					</div>
				</header>

				{#if app?.workspacePath}
					<section class="dash-section clusters-section">
						<h3><svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1 0 2.83 2 2 0 0 1-2.83 0l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-2 2 2 2 0 0 1-2-2v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83 0 2 2 0 0 1 0-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1-2-2 2 2 0 0 1 2-2h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 0-2.83 2 2 0 0 1 2.83 0l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 2-2 2 2 0 0 1 2 2v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 0 2 2 0 0 1 0 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 2 2 2 2 0 0 1-2 2h-.09a1.65 1.65 0 0 0-1.51 1z"/></svg>clusters</h3>
						
						{#if commonplaces.length > 0}
							<div class="work-areas-grid">
								{#each commonplaces as cluster}
									<!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
									<div class="wa-card" onclick={() => openCluster(cluster)}>
										<div class="wa-title">
											Cluster {commonplaces.indexOf(cluster) + 1}
										</div>
										<div class="wa-balance">{cluster.length} notes connected</div>
									</div>
								{/each}
							</div>
						{:else}
							<div style="font-size: 0.85rem; color: var(--neutral-600); padding: 1rem 0;">
								No clusters found. Link notes together to form connections!
							</div>
						{/if}
					</section>
				{/if}

				<div class="dashboard-grid">
					<!-- Left Column -->
					<div class="dash-left">


						<div class="dash-split">
							<!-- Tasks -->
							<section class="dash-section">
								<div class="section-header-split">
									<h3><svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 11 12 14 22 4"/><path d="M21 12v7a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h11"/></svg>tasks</h3>
								</div>
								<div class="dash-task-list">
									{#each dashTasks as task (task.id)}
										<label class="dash-task">
											<input type="checkbox" bind:checked={task.done} /> 
											<span class:done={task.done}>{task.text}</span>
											<button class="remove-task-btn" onclick={(e) => { e.preventDefault(); removeTask(task.id); }} aria-label="Remove task">&times;</button>
										</label>
									{/each}
								</div>
								<form class="add-task-form" onsubmit={(e) => { e.preventDefault(); addTask(); }}>
									<input type="text" placeholder="Add a new task..." bind:value={newTaskText} />
									<button type="submit" class="btn-primary" style="padding: 4px 12px; border-radius: var(--radius-xs); font-size: 0.8rem; font-weight: 500; min-height: unset; line-height: 1;">Add</button>
								</form>
							</section>

							<!-- Notes -->
							<section class="dash-section" id="sec-contents">
								<div class="section-header-split">
									<h3><svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><rect x="3" y="3" width="18" height="18" rx="2"/><line x1="9" y1="3" x2="9" y2="21"/><line x1="15" y1="3" x2="15" y2="21"/></svg>notes</h3>
									<div class="row-menu-wrap" style="position: relative;">
										<button class="btn-ghost" onclick={(e) => { e.stopPropagation(); activeMenuId = activeMenuId === 'new-note-menu' ? null : 'new-note-menu'; }} disabled={isBusy}>
											New note ▾
										</button>
										{#if activeMenuId === 'new-note-menu'}
											<div class="row-dropdown" style="top: 100%; right: 0; margin-top: 4px; z-index: 10;">
												<button class="row-delete" onclick={(e) => { e.preventDefault(); createNote('md'); activeMenuId = null; }} style="color: var(--text-primary); text-align: left;">Markdown Note</button>
												<button class="row-delete" onclick={(e) => { e.preventDefault(); createNote('tex'); activeMenuId = null; }} style="color: var(--text-primary); text-align: left;">LaTeX Document</button>
												<button class="row-delete" onclick={(e) => { e.preventDefault(); createNote('ipynb'); activeMenuId = null; }} style="color: var(--text-primary); text-align: left;">Jupyter Notebook</button>
											</div>
										{/if}
									</div>
								</div>
								<div class="contents-single-list">
									{#each dashNotes as note}
										<!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
										<div class="kanban-card" onclick={() => openNote(note.id)}>
											<div class="kc-header-row">
												<div style="display: flex; align-items: flex-start; gap: 8px;">
													<div class="kc-title">{note.title}</div>
													<div class="row-badge">
														{getNoteBadge(note)}
													</div>
												</div>
												<div class="row-menu-wrap">
													<button class="row-menu-btn" onclick={(e) => { e.stopPropagation(); activeMenuId = activeMenuId === note.id ? null : note.id; }} aria-label="Options">
														<svg width="12" height="12" viewBox="0 0 24 24" fill="currentColor"><circle cx="12" cy="5" r="1.5"/><circle cx="12" cy="12" r="1.5"/><circle cx="12" cy="19" r="1.5"/></svg>
													</button>
													{#if activeMenuId === note.id}
														<!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
														<div class="row-dropdown" onclick={(e) => e.stopPropagation()}>
															<button class="row-delete" onclick={(e) => { e.preventDefault(); togglePin(note.id); }} style="color: var(--text-primary);">
																{pinnedNoteIds.includes(note.id) ? 'Unpin' : 'Pin'}
															</button>
															<button class="row-delete" onclick={(e) => requestDeleteNote(e, note.id)}>Delete</button>
														</div>
													{/if}
												</div>
											</div>
											<div class="kc-snippet">{note.excerpt}</div>
											<div class="kc-tags">
												{#each note.tags.slice(0, 3) as tag}
													<span class="kc-tag">#{tag}</span>
												{/each}
											</div>
											<div class="kc-footer">
												<div class="kc-platform">{note.relativePath}</div>
												<div class="kc-dates">
													<span>Created: {new Date(note.createdAt).toLocaleDateString()}</span>
													<span>Modified: {new Date(note.updatedAt).toLocaleDateString()}</span>
												</div>
											</div>
										</div>
									{/each}
								</div>
							</section>
						</div>

						<!-- Pinned Notes -->
						<section class="dash-section pinned-section" style="width: 100%;">
							<div class="section-header-split">
								<h3><svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M19 21l-7-5-7 5V5a2 2 0 0 1 2-2h10a2 2 0 0 1 2 2z"/></svg>pinned notes</h3>
							</div>
							<div class="pinned-list">
								{#each pinnedNotes as note}
									<!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
									<div class="pinned-item" onclick={() => openNote(note.id)}>
										<div class="pi-title">{note.title}</div>
										<div class="pi-date">
											{new Date(note.createdAt).toLocaleDateString('en-US', { month: 'long', day: 'numeric', year: 'numeric' })} 
											{new Date(note.createdAt).toLocaleTimeString('en-US', { hour: '2-digit', minute: '2-digit', hour12: false })}
										</div>
										<div class="pi-badge">
											{getNoteBadge(note)}
										</div>
									</div>
								{/each}
								{#if pinnedNotes.length === 0}
									<div style="font-size: 0.8rem; color: var(--neutral-600); margin-top: 1rem;">No pinned notes.</div>
								{/if}
							</div>
						</section>
					</div>

					<!-- Right Column (Timeline) -->
					{#if showTimeline}
					<div class="dash-right">
						<section class="dash-section timeline-widget">
							<div class="section-header-split" style="border-bottom: none;">
								<h3><svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="10"/><polyline points="12 6 12 12 16 14"/></svg>timeline</h3>
								<div class="tl-subtitle" style="margin-top: 0; font-family: var(--font-mono); font-size: 0.75rem;">{new Date().toLocaleDateString('en-US', { month: 'long', day: 'numeric' })}</div>
							</div>
							<div class="tl-track">
								{#if timelineNotes.length === 0}
									<div class="tl-empty" style="color: var(--neutral-600); font-size: 0.8rem; margin-top: 1rem;">No activity today.</div>
								{:else}
									{#each timelineNotes as note, i}
										<div class="tl-item {i === 0 ? 'is-active' : ''}">
											<div class="tl-time-side">
												{new Date(note.createdAt).toLocaleTimeString([], {hour: '2-digit', minute:'2-digit', hour12: false})}
											</div>
											<div class="tl-node">
												<div class="tl-circle">
													{#if i === 0}
														<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round" width="10" height="10"><polyline points="20 6 9 17 4 12"></polyline></svg>
													{/if}
												</div>
												{#if i < timelineNotes.length - 1}
													<div class="tl-line"></div>
												{/if}
											</div>
											<!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
											<div class="tl-content-side" onclick={() => openNote(note.id)} style="cursor:pointer">
												<div class="tl-title" class:active-title={i === 0}>{note.title}</div>
												<div class="tl-subtext">{note.relativePath}</div>
											</div>
										</div>
									{/each}
								{/if}
							</div>
						</section>
					</div>
					{/if}
				</div>
				<section class="dash-section notebook-section" style="margin-top: auto; padding-top: 2rem;">
					<h3><svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="M4 19.5A2.5 2.5 0 0 1 6.5 17H20"/><path d="M6.5 2H20v20H6.5A2.5 2.5 0 0 1 4 19.5v-15A2.5 2.5 0 0 1 6.5 2z"/></svg>workspace</h3>
					<div class="notebook-list">
						<div class="nb-item">
							<div class="nb-title">{app?.workspacePath ? workspaceLabel(app.workspacePath) : 'No workspace'}</div>
							<div class="nb-date"><span class="kc-tag">active</span> {app?.workspacePath || ''}</div>
						</div>
					</div>
				</section>
			</div>
		{/if}
	</main>
</div>

{#if isClusterDialogOpen}
	<!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
	<div class="modal-overlay" onclick={closeClusterDialog}>
		<div class="modal-content" onclick={(e) => e.stopPropagation()}>
			<header class="modal-header">
				<h2>Cluster Notes ({selectedCluster.length})</h2>
				<p class="modal-subtitle">Select a note to view its contents.</p>
			</header>
			<div class="modal-body">
				<div class="table-list">
					<div class="table-header">
						<div class="th-col">Title</div>
						<div class="th-col">Folder</div>
						<div class="th-col">Created</div>
						<div class="th-col">Modified</div>
					</div>
					{#each selectedCluster as note}
						<!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
						<div class="table-row" onclick={() => { closeClusterDialog(); openNote(note.id); }}>
							<div class="td-col td-primary">{note.title}</div>
							<div class="td-col">{note.folder || 'root'}</div>
							<div class="td-col">{new Date(note.createdAt).toLocaleDateString()}</div>
							<div class="td-col">{new Date(note.updatedAt).toLocaleDateString()}</div>
						</div>
					{/each}
				</div>
			</div>
			<footer class="modal-footer">
				<button class="btn-cancel" onclick={closeClusterDialog}>Cancel</button>
			</footer>
		</div>
	</div>
{/if}

<style>
	/* ── Layout ── */
	.shell {
		display: grid;
		grid-template-columns: 300px 1fr;
		height: calc(100vh - 32px);
		overflow: hidden;
		animation: fade-in 0.2s ease-out;
		transition: grid-template-columns 0.3s cubic-bezier(0.4, 0, 0.2, 1);
		position: relative;
	}
	.shell.rail-collapsed {
		grid-template-columns: 0px 1fr;
	}

	/* ── Rail ── */
	.rail {
		display: flex;
		flex-direction: column;
		border-right: 1px solid var(--border-default);
		background: var(--bg-panel);
		overflow: hidden;
		font-family: var(--font-mono);
		transition: transform 0.3s cubic-bezier(0.4, 0, 0.2, 1);
	}

	@media (max-width: 1200px) {
		.shell {
			grid-template-columns: 1fr !important;
		}
		.rail {
			position: absolute;
			top: 0;
			left: 0;
			bottom: 0;
			width: 300px;
			max-width: 85vw;
			z-index: 100;
			transform: translateX(0);
			box-shadow: 4px 0 24px rgba(0,0,0,0.8);
		}
		.shell.rail-collapsed .rail {
			transform: translateX(-100%);
			box-shadow: none;
		}
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
		font-size: 1.35rem;
		font-weight: 700;
		letter-spacing: -0.03em;
		color: var(--text-hero);
	}

	.rail-search {
		position: relative;
		margin: 0 var(--space-4) var(--space-4);
		flex-shrink: 0;
	}
	.search-icon {
		position: absolute;
		left: 12px;
		top: 50%;
		transform: translateY(-50%);
		color: var(--text-secondary);
		pointer-events: none;
	}
	.search-input {
		width: 100%;
		box-sizing: border-box;
		padding: 10px 12px 10px 36px;
		font-size: 1rem;
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
		font-size: 1rem;
		color: var(--text-secondary);
		padding: var(--space-4) var(--space-3);
		margin: 0;
	}

	.section-label {
		display: flex;
		justify-content: space-between;
		align-items: center;
		padding: var(--space-3) var(--space-3);
		font-size: 0.82rem;
		font-weight: 700;
		letter-spacing: 0.08em;
		text-transform: uppercase;
		color: #ffffff;
		user-select: none;
	}
	.section-count {
		font-size: 0.82rem;
		color: #ffffff;
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
		font-size: 1.05rem;
		color: var(--text-primary);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
		line-height: 1.3;
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

	.row-time {
		flex-shrink: 0;
		font-size: 0.85rem;
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
		font-size: 0.9rem;
		font-family: var(--font-mono);
		background: transparent;
		color: #e05555;
		border: none;
		border-radius: var(--radius-xs);
		cursor: pointer;
	}
	.row-delete:hover { background: rgba(224,85,85,0.1); }

	/* ── Rail details panel ── */
	.ov-group {
		display: flex;
		flex-direction: column;
		padding: 0 var(--space-3);
	}
	.ov-row {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--space-3);
		width: 100%;
		box-sizing: border-box;
		padding: 8px 0;
		font-size: 1rem;
		font-family: var(--font-mono);
		background: transparent;
		border: none;
		border-bottom: 1px solid var(--neutral-1000);
		color: var(--text-secondary);
		text-align: left;
	}
	.ov-group .ov-row:last-child { border-bottom: none; }
	.ov-key {
		display: flex;
		align-items: center;
		gap: var(--space-2);
		min-width: 0;
	}
	.ov-key svg { flex-shrink: 0; color: var(--neutral-600); }
	.ov-ellipsis {
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}
	.ov-val {
		flex-shrink: 0;
		color: var(--text-primary);
		font-size: 0.95rem;
	}
	.ov-val.ov-ok { color: #4caf50; }
	.ov-clickable { cursor: pointer; }
	.ov-clickable:hover .ov-key { color: var(--text-primary); }

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
		font-size: 0.95rem;
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
		background-color: var(--bg-page);
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

	.workspace-empty {
		height: 100%;
		display: flex;
		flex-direction: column;
		align-items: center;
		justify-content: center;
		gap: var(--space-3);
		font-family: var(--font-mono);
	}
	.workspace-empty p {
		font-size: 1rem;
		color: var(--neutral-600);
		margin: 0;
	}
	.ws-status-line {
		margin: 0;
		font-size: 0.8rem;
		color: var(--neutral-500);
	}

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



	/* ── Dashboard Styles ── */
	.dashboard-container {
		display: flex;
		flex-direction: column;
		height: 100%;
		padding: 1.5rem 2rem;
		box-sizing: border-box;
		font-family: var(--font-mono);
		overflow-y: auto;
		color: var(--text-primary);
	}

	.dashboard-header {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--space-4);
		margin-bottom: var(--space-4);
	}
	.dashboard-header h2 {
		margin: 0;
		font-size: 2.2rem;
		font-weight: 800;
		letter-spacing: -0.04em;
		color: var(--text-hero);
	}
	.dash-tabs {
		display: flex;
		gap: var(--space-2);
		margin-top: 6px;
	}
	.dash-tab {
		display: flex;
		align-items: center;
		gap: 6px;
		background: transparent;
		border: 1px solid transparent;
		color: var(--text-secondary);
		padding: 4px 8px;
		font-size: 0.8rem;
		font-family: var(--font-mono);
		cursor: pointer;
		border-radius: var(--radius-xs);
		transition: color 0.15s, background 0.15s;
	}
	.dash-tab:hover { color: var(--text-primary); }
	.dash-tab.active {
		color: var(--text-primary);
		background: rgba(255,255,255,0.06);
		border-color: var(--border-default);
	}

	.dashboard-grid {
		display: flex;
		gap: 3rem;
		align-items: stretch;
		flex: 1;
		min-height: 0;
	}
	.dash-left {
		flex: 1;
		display: flex;
		flex-direction: column;
		gap: 1.5rem;
		min-height: 0;
	}
	.dash-right {
		width: 320px;
		flex-shrink: 0;
		display: flex;
		flex-direction: column;
		gap: 2rem;
		min-height: 0;
	}

	.dash-section {
		min-width: 0;
	}
	.dash-section h3 {
		display: flex;
		align-items: center;
		gap: 8px;
		font-size: 0.95rem;
		font-weight: 700;
		text-transform: uppercase;
		letter-spacing: 0.08em;
		color: #ffffff;
		margin: 0 0 0.5rem 0;
		border-bottom: 1px solid var(--border-default);
		padding-bottom: 6px;
	}
	.dash-section h3 svg { color: var(--neutral-600); }

	.work-areas-grid {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(180px, 1fr));
		gap: var(--space-3);
		margin-top: 1rem;
	}
	.wa-card {
		background: var(--bg-panel);
		border: 1px solid var(--border-default);
		border-radius: var(--radius-sm);
		padding: var(--space-3);
		display: flex;
		flex-direction: column;
		gap: 12px;
		transition: border-color 0.15s, background 0.15s;
	}
	.wa-card:hover { border-color: var(--neutral-600); background: rgba(255,255,255,0.03); }
	.wa-title {
		display: flex;
		align-items: center;
		gap: 8px;
		font-size: 0.95rem;
		font-weight: 600;
	}
	.wa-icon {
		display: flex;
		align-items: center;
		justify-content: center;
		width: 18px;
		height: 18px;
		border-radius: 4px;
		font-size: 0.65rem;
		font-weight: 800;
		color: #fff;
	}
	.wa-balance {
		font-size: 0.75rem;
		color: var(--text-secondary);
	}

	.dash-split {
		display: grid;
		grid-template-columns: minmax(0, 1fr) minmax(0, 2.5fr);
		gap: 1.5rem;
		height: 480px;
		min-height: 0;
		max-height: 480px;
	}
	.dash-split > .dash-section {
		display: flex;
		flex-direction: column;
		min-height: 0;
	}
	.section-header-split {
		display: flex;
		justify-content: space-between;
		align-items: center;
		margin-bottom: 1rem;
		border-bottom: 1px solid var(--border-default);
		padding-bottom: 6px;
		height: 32px;
		box-sizing: border-box;
		margin-top: 0;
	}
	.section-header-split h3 {
		margin: 0;
		border-bottom: none;
		padding-bottom: 0;
		line-height: 1;
	}

	.dash-task-list {
		display: flex;
		flex-direction: column;
		gap: 2px;
		flex: 1;
		min-height: 0;
		overflow-y: auto;
		padding-right: 8px;
	}
	.dash-task-list::-webkit-scrollbar { width: 4px; }
	.dash-task-list::-webkit-scrollbar-thumb { background: var(--border-default); border-radius: 4px; }
	.dash-task {
		display: flex;
		align-items: flex-start;
		gap: 10px;
		padding: 6px 8px;
		border-radius: var(--radius-xs);
		cursor: pointer;
		font-size: 0.85rem;
		color: var(--text-secondary);
		line-height: 1.4;
		transition: background 0.1s, color 0.1s;
		position: relative;
	}
	.dash-task:hover {
		background: rgba(255,255,255,0.04);
		color: var(--text-primary);
	}
	.dash-task input {
		margin-top: 3px;
		accent-color: var(--accent-300);
	}
	.dash-task span.done {
		text-decoration: line-through;
		color: var(--neutral-600);
	}
	.remove-task-btn {
		position: absolute;
		right: 8px;
		top: 50%;
		transform: translateY(-50%);
		background: transparent;
		border: none;
		color: var(--text-secondary);
		cursor: pointer;
		opacity: 0;
		font-size: 1.2rem;
		line-height: 1;
		transition: opacity 0.1s;
	}
	.dash-task:hover .remove-task-btn { opacity: 1; }
	.remove-task-btn:hover { color: #e05555; }

	.add-task-form {
		margin-top: var(--space-3);
		display: flex;
		gap: 8px;
	}
	.add-task-form input {
		flex: 1;
		width: 100%;
		margin: 0;
		min-width: 0;
		box-sizing: border-box;
		background: transparent;
		border: 1px solid var(--border-default);
		color: var(--text-primary);
		border-radius: var(--radius-xs);
		padding: 4px 8px;
		font-family: var(--font-mono);
		font-size: 0.8rem;
		outline: none;
	}
	.add-task-form input:focus { border-color: var(--neutral-600); }

	.contents-single-list {
		display: flex;
		flex-direction: column;
		gap: var(--space-2);
		flex: 1;
		min-height: 0;
		overflow-y: auto;
		padding-right: 8px;
	}
	.contents-single-list::-webkit-scrollbar { width: 4px; }
	.contents-single-list::-webkit-scrollbar-thumb { background: var(--border-default); border-radius: 4px; }

	.kanban-board {
		display: flex;
		gap: var(--space-4);
	}
	.kanban-col {
		flex: 1;
		display: flex;
		flex-direction: column;
		gap: var(--space-3);
	}
	.kanban-col h4 {
		margin: 0;
		font-size: 0.85rem;
		font-weight: 600;
		color: var(--text-secondary);
		display: flex;
		align-items: center;
		gap: 6px;
	}
	.kanban-count {
		font-size: 0.7rem;
		color: var(--neutral-600);
		background: rgba(255,255,255,0.05);
		padding: 2px 6px;
		border-radius: 10px;
	}
	.kanban-card {
		position: relative;
		cursor: pointer;
		background: #262626;
		border: 1px solid transparent;
		border-radius: var(--radius-sm);
		padding: var(--space-2) var(--space-3);
		display: flex;
		flex-direction: column;
		gap: 6px;
		transition: background 0.15s;
	}
	.kanban-card:hover { background: #333; }
	.kanban-card:hover .row-menu-btn { opacity: 1; }
	.kanban-card-progress { border-color: rgba(100,181,246,0.3); background: rgba(100,181,246,0.03); }
	.kanban-card-complete { border-color: rgba(129,199,132,0.3); background: rgba(129,199,132,0.03); }
	
	.kc-header-row {
		display: flex;
		justify-content: space-between;
		align-items: flex-start;
		gap: 8px;
	}
	.kc-title {
		font-size: 0.85rem;
		line-height: 1.3;
		color: var(--text-primary);
	}
	.kc-snippet {
		font-size: 0.8rem;
		color: var(--text-secondary);
		display: -webkit-box;
		-webkit-line-clamp: 2;
		-webkit-box-orient: vertical;
		overflow: hidden;
		line-height: 1.4;
		word-break: break-word;
		white-space: normal;
	}
	.kc-dates {
		display: flex;
		gap: var(--space-3);
		font-size: 0.7rem;
		color: var(--neutral-500);
	}
	.kc-tags { display: flex; flex-wrap: wrap; gap: 4px; }
	.kc-tag {
		font-size: 0.65rem;
		padding: 2px 6px;
		background: rgba(255,255,255,0.08);
		border: 1px solid var(--border-default);
		border-radius: var(--radius-xs);
		color: var(--text-secondary);
	}
	.kc-platform {
		display: flex;
		align-items: center;
		gap: 6px;
		font-size: 0.75rem;
		color: var(--text-secondary);
	}
	.kc-footer {
		display: flex;
		align-items: center;
		justify-content: space-between;
		margin-top: 2px;
	}

	.notebook-list {
		display: flex;
		flex-direction: column;
	}
	.nb-item {
		display: flex;
		justify-content: space-between;
		align-items: center;
		padding: var(--space-2) 0;
		border-bottom: 1px solid var(--neutral-1000);
	}
	.nb-item:last-child { border-bottom: none; }
	.nb-title { font-size: 0.9rem; color: var(--text-secondary); }
	.nb-date { font-size: 0.75rem; color: var(--neutral-600); display: flex; align-items: center; gap: 8px; }

	/* Calendar Widget */
	.calendar-widget {
		background: transparent;
		border: none;
		border-radius: var(--radius-md);
		padding: var(--space-4);
	}
	.cal-header {
		display: flex;
		justify-content: space-between;
		align-items: center;
		margin-bottom: var(--space-4);
	}
	.cal-month {
		font-size: 1rem;
		font-weight: 700;
		background: rgba(255,255,255,0.05);
		padding: 4px 12px;
		border-radius: 12px;
	}
	.cal-nav {
		background: transparent;
		border: none;
		color: var(--text-secondary);
		cursor: pointer;
		padding: 4px;
	}
	.cal-nav:hover { color: var(--text-primary); }
	.cal-grid {
		display: grid;
		grid-template-columns: repeat(7, 1fr);
		gap: 8px 4px;
		text-align: center;
	}
	.cal-day-name {
		font-size: 0.7rem;
		color: var(--neutral-600);
		font-weight: 600;
		margin-bottom: 4px;
	}
	.cal-day {
		font-size: 0.85rem;
		color: var(--text-secondary);
		padding: 6px 0;
		border-radius: 50%;
		cursor: pointer;
	}
	.cal-day:hover { background: rgba(255,255,255,0.05); color: var(--text-primary); }
	.cal-day.active {
		background: var(--accent-300);
		color: #000;
		font-weight: 700;
	}
	.w-full { width: 100%; }
	.mt-4 { margin-top: 1rem; }

	/* Timeline Widget */
	.timeline-widget {
		background: transparent;
		border: none;
		border-radius: var(--radius-md);
		flex: 1;
		display: flex;
		flex-direction: column;
		height: 100%;
		min-height: 0;
	}
	.tl-subtitle { color: var(--text-secondary); margin: 0; line-height: 1; }
	.tl-track {
		display: flex;
		flex-direction: column;
		flex: 1;
		min-height: 0;
		overflow-y: auto;
		padding-right: 8px;
	}
	.tl-track::-webkit-scrollbar { width: 4px; }
	.tl-track::-webkit-scrollbar-thumb { background: var(--border-default); border-radius: 4px; }
	.tl-item {
		display: flex;
		align-items: stretch;
		min-height: 60px;
		flex-shrink: 0;
	}
	.tl-time-side {
		width: 65px;
		flex-shrink: 0;
		font-size: 0.7rem;
		color: var(--text-secondary);
		padding-top: 1px;
		text-align: left;
		padding-right: 8px;
	}
	.is-active .tl-time-side {
		color: var(--text-primary);
	}
	.tl-node {
		display: flex;
		flex-direction: column;
		align-items: center;
		width: 20px;
		flex-shrink: 0;
		margin-right: 16px;
	}
	.tl-circle {
		width: 18px;
		height: 18px;
		border-radius: 50%;
		border: 2px solid var(--border-default);
		background: transparent;
		display: flex;
		align-items: center;
		justify-content: center;
		color: transparent;
		z-index: 2;
	}
	.is-active .tl-circle {
		background: var(--accent-300);
		border-color: var(--accent-300);
		color: #000;
	}
	.tl-line {
		flex: 1;
		width: 2px;
		background: var(--border-default);
		margin: 4px 0;
		border-radius: 1px;
	}
	.is-active .tl-line {
		background: transparent;
		border-left: 2px dashed var(--accent-300);
		width: 0;
	}
	.tl-content-side {
		flex: 1;
		padding-bottom: 24px;
	}
	.tl-title { 
		font-size: 0.9rem; 
		font-weight: 500; 
		color: var(--text-secondary); 
		margin-bottom: 4px;
		line-height: 1.2;
	}
	.active-title {
		color: var(--text-primary);
	}
	.tl-subtext { 
		font-size: 0.75rem; 
		color: var(--neutral-600); 
	}

	/* ── Clusters Modal ── */
	.clusters-section { margin-bottom: 2rem; }
	.modal-overlay {
		position: fixed;
		top: 0; left: 0; right: 0; bottom: 0;
		background: rgba(0,0,0,0.6);
		backdrop-filter: blur(2px);
		display: flex;
		align-items: center;
		justify-content: center;
		z-index: 1000;
		animation: fade-in 0.2s ease-out;
	}
	.modal-content {
		background: #151515;
		border: 1px solid var(--border-default);
		border-radius: var(--radius-sm);
		width: 90%;
		max-width: 700px;
		max-height: 80vh;
		display: flex;
		flex-direction: column;
		box-shadow: 0 15px 40px rgba(0,0,0,0.8);
	}
	.modal-header {
		display: flex;
		flex-direction: column;
		gap: 6px;
		padding: 1.5rem 1.5rem 1rem;
	}
	.modal-header h2 { margin: 0; font-size: 1.15rem; color: var(--text-primary); font-weight: 700; }
	.modal-subtitle { margin: 0; font-size: 0.85rem; color: var(--text-secondary); }
	
	.modal-body {
		padding: 0;
		overflow-y: auto;
	}

	.modal-footer {
		display: flex;
		justify-content: flex-end;
		padding: 1rem 1.5rem 1.5rem;
		border-top: 1px solid var(--border-default);
	}
	.btn-cancel {
		background: transparent;
		border: 1px solid var(--border-default);
		color: var(--text-primary);
		padding: 6px 14px;
		border-radius: 4px;
		font-size: 0.85rem;
		cursor: pointer;
		font-family: var(--font-mono);
		transition: background 0.15s, border-color 0.15s;
	}
	.btn-cancel:hover {
		background: rgba(255,255,255,0.05);
		border-color: var(--neutral-600);
	}

	/* ── Table List ── */
	.table-list {
		display: flex;
		flex-direction: column;
		width: 100%;
	}
	.table-header {
		display: grid;
		grid-template-columns: 2fr 1fr 1fr 1fr;
		gap: 1.5rem;
		padding: 0.75rem 1.5rem;
		border-bottom: 1px solid var(--border-default);
		color: var(--text-primary);
		font-weight: 600;
		font-size: 0.85rem;
	}
	.table-row {
		display: grid;
		grid-template-columns: 2fr 1fr 1fr 1fr;
		gap: 1.5rem;
		padding: 0.75rem 1.5rem;
		border-bottom: 1px solid var(--border-default);
		color: var(--text-secondary);
		font-size: 0.85rem;
		align-items: center;
		cursor: pointer;
		transition: background 0.15s;
	}
	.btn-ghost:hover {
		background: rgba(255, 255, 255, 0.05);
	}

	/* Pinned Section */
	.pinned-section {
		display: flex;
		flex-direction: column;
		min-height: 0;
	}
	.pinned-list {
		display: flex;
		flex-direction: column;
		gap: 2px;
		overflow-y: auto;
		padding-right: 8px;
	}
	.pinned-list::-webkit-scrollbar { width: 4px; }
	.pinned-list::-webkit-scrollbar-thumb { background: var(--border-default); border-radius: 4px; }
	
	.pinned-item {
		display: flex;
		align-items: center;
		padding: 3px 0;
		cursor: pointer;
		font-size: 0.85rem;
		border-radius: 4px;
	}
	.pinned-item:hover .pi-title {
		color: var(--text-primary);
	}
	.pi-title {
		flex: 1;
		color: var(--text-secondary);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
		font-weight: 500;
		transition: color 0.15s ease;
	}
	.pi-date {
		color: var(--neutral-600);
		margin-left: 16px;
		font-family: var(--font-mono);
		font-size: 0.7rem;
		flex-shrink: 0;
		width: 175px;
		text-align: right;
	}
	.pi-badge {
		margin-left: 12px;
		border: 1px solid var(--border-default);
		border-radius: 4px;
		padding: 2px 6px;
		font-size: 0.65rem;
		color: var(--neutral-400);
		font-family: var(--font-mono);
		background: rgba(255, 255, 255, 0.02);
		flex-shrink: 0;
		width: 80px;
		text-align: center;
		box-sizing: border-box;
	}

	.contents-single-list .table-row:last-child {
		border-bottom: none;
	}
	.table-row:hover {
		background: rgba(255,255,255,0.02);
	}
	.table-row:last-child {
		border-bottom: none;
	}
	.td-primary {
		color: var(--text-primary);
		font-weight: 500;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}
	.td-col {
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}
	.th-col {
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.header-icon-btn {
		display: flex;
		align-items: center;
		justify-content: center;
		background: transparent;
		border: 1px solid transparent;
		border-radius: var(--radius-sm);
		padding: 6px;
		color: var(--text-secondary);
		cursor: pointer;
		transition: background 0.15s, color 0.15s;
	}
	.header-icon-btn:hover {
		background: rgba(255,255,255,0.05);
		color: var(--text-primary);
	}
	.header-icon-btn.active {
		color: var(--accent-100);
		background: rgba(255,255,255,0.02);
	}

	.header-search-btn {
		display: flex;
		align-items: center;
		gap: 8px;
		background: #1e1e1e;
		border: 1px solid var(--border-default);
		border-radius: var(--radius-sm);
		padding: 8px 12px;
		color: var(--text-secondary);
		font-family: var(--font-mono);
		font-size: 0.85rem;
		cursor: pointer;
		width: 250px;
		transition: border-color 0.15s, color 0.15s;
	}
	.header-search-btn:hover {
		border-color: var(--neutral-600);
		color: var(--text-primary);
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
</style>
