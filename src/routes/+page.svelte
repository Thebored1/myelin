<script lang="ts">
	import { invoke } from '@tauri-apps/api/core';
	import { listen } from '@tauri-apps/api/event';
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { open } from '@tauri-apps/plugin-dialog';
	import { sidebarOpen } from '$lib/stores';
	import { theme, toggleTheme } from '$lib/theme';
	import { appCache } from '$lib/appCache';
	import type {
		AppSnapshot,
		NoteDocument,
		NoteSummary,
		ProviderStatus,
		SearchResponse
	} from '$lib/types';
	import { onMount } from 'svelte';
	import { getVersion } from '@tauri-apps/api/app';

	let appVersion = $state('');
	let app = $state<AppSnapshot | null>(null);
	// True once the initial snapshot has loaded — prevents the "no workspace"
	// welcome screen from flashing before we know if a workspace is connected.
	let ready = $state(false);
	let indexing = $state(false);
	let provider = $state<ProviderStatus | null>(null);
	let query = $state('');
	let isBusy = $state(false);
	let message = $state('');
	let searchResults = $state<SearchResponse | null>(null);
	let pendingCreateCount = 0;
	let createLoopRunning = false;
	let activeMenuId = $state<string | null>(null);
	let showAddMenu = $state(false);
	let deleteDialog: HTMLDialogElement | undefined = $state();
	let noteToDelete = $state<string | null>(null);
	let notebookDialog: HTMLDialogElement | undefined = $state();
	let newNotebookName = $state('');

	let dashTasks = $state<{id: number, text: string, done: boolean}[]>([]);
	let currentWorkspaceForTasks = $state<string | null>(null);
	let pinnedNoteIds = $state<string[]>([]);
	let showTimeline = $state(true);
	let tasksCollapsed = $state(false);

	$effect(() => {
		if (app?.workspacePath && app.workspacePath !== currentWorkspaceForTasks) {
			currentWorkspaceForTasks = app.workspacePath;
			activeTag = null;
			selectedNote = null;
			activeNotebook = null;
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
			tasksCollapsed = localStorage.getItem(`taskscollapsed_${app.workspacePath}`) === 'true';
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
			localStorage.setItem(`taskscollapsed_${currentWorkspaceForTasks}`, tasksCollapsed.toString());
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

	// ── Notes list (left) — filter ──
	// Main-pane tabs filter by category ("notes" = editable md/tex/ipynb, "documents"
	// = source pdf/epub); the sidebar dropdowns set a specific type. Both share this.
	type NbFilter = 'all' | 'notes' | 'documents' | 'md' | 'tex' | 'ipynb' | 'pdf' | 'epub';
	let activeTypeFilter = $state<NbFilter>('all');

	function noteType(n: NoteSummary): 'md' | 'pdf' | 'tex' | 'ipynb' | 'epub' {
		const rel = n.relativePath.toLowerCase();
		if (rel.endsWith('.pdf')) return 'pdf';
		if (rel.endsWith('.epub')) return 'epub';
		if (rel.endsWith('.tex')) return 'tex';
		if (rel.endsWith('.ipynb')) return 'ipynb';
		return 'md';
	}
	const NOTE_GROUP = ['md', 'tex', 'ipynb'];
	const DOC_GROUP = ['pdf', 'epub'];

	// Sidebar "notes" / "documents" groups. "notes" = editable working docs (right
	// editor pane); "documents" = uploaded source material (left pane).
	const NOTE_SUBTYPES = [
		{ type: 'md', label: 'markdown' },
		{ type: 'tex', label: 'latex' },
		{ type: 'ipynb', label: 'jupyter' }
	] as const;
	const DOC_SUBTYPES = [
		{ type: 'pdf', label: 'pdf' },
		{ type: 'epub', label: 'epub' }
	] as const;
	let typeCounts = $derived.by(() => {
		const c: Record<string, number> = { md: 0, tex: 0, ipynb: 0, pdf: 0, epub: 0 };
		for (const n of dashNotes) c[noteType(n)] += 1;
		return c;
	});
	let notesExpanded = $state(false);
	let docsExpanded = $state(false);
	let notebooksExpanded = $state(true);
	let showNotebookMenu = $state(false);
	let isClustersListOpen = $state(false);
	function openClustersDialog() {
		isClustersListOpen = true;
	}
	function closeClustersList() {
		isClustersListOpen = false;
	}

	// Sidebar-driven narrowing of the same notes list: a tag.
	let activeTag = $state<string | null>(null);
	let selectedNote = $state<NoteSummary | null>(null);
	let notebooks = $state<string[]>([]);
	let activeNotebook = $state<string | null>(null);

	// Filter by category/type + tag, float pinned notes to the top, keep recency order.
	let filteredNotebook = $derived.by(() => {
		let base = dashNotes;
		const f = activeTypeFilter;
		if (f === 'notes') base = base.filter((n) => NOTE_GROUP.includes(noteType(n)));
		else if (f === 'documents') base = base.filter((n) => DOC_GROUP.includes(noteType(n)));
		else if (f !== 'all') base = base.filter((n) => noteType(n) === f);
		if (activeTag !== null) base = base.filter((n) => n.tags.includes(activeTag!));
		// null notebook = "uncategorized" (notes not in any notebook / workspace root).
		if (activeNotebook === null) base = base.filter((n) => notebookOf(n) === null);
		else base = base.filter((n) => n.folder === activeNotebook || n.folder.startsWith(activeNotebook + '/'));
		return [...base].sort(
			(a, b) =>
				(pinnedNoteIds.includes(b.id) ? 1 : 0) - (pinnedNoteIds.includes(a.id) ? 1 : 0)
		);
	});

	function setTypeFilter(t: typeof activeTypeFilter) {
		activeTypeFilter = t;
	}
	function toggleTag(tag: string) {
		activeTag = activeTag === tag ? null : tag;
	}

	function fullDateTime(value: string) {
		const d = new Date(value);
		return (
			d.toLocaleDateString('en-US', { month: 'long', day: 'numeric', year: 'numeric' }) +
			' ' +
			d.toLocaleTimeString('en-US', { hour: 'numeric', minute: '2-digit' })
		);
	}

	// ── Tasks (right) — filter tabs ──
	let activeTaskFilter = $state<'all' | 'active' | 'done'>('all');
	let filteredTasks = $derived.by(() => {
		if (activeTaskFilter === 'active') return dashTasks.filter((t) => !t.done);
		if (activeTaskFilter === 'done') return dashTasks.filter((t) => t.done);
		return dashTasks;
	});

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
		appCache.app = app;
		appCache.provider = provider;
		void loadNotebooks();
	}

	// Keep the cross-navigation cache in sync with any path that reassigns the
	// snapshot (create / delete / rebuild / workspace change), so a later return
	// to Home paints current data, not a stale copy.
	$effect(() => {
		if (app) appCache.app = app;
		if (provider) appCache.provider = provider;
	});

	function folderFromRelativePath(relativePath: string) {
		const segments = relativePath.split('/').filter(Boolean);
		return segments.length > 1 ? segments.slice(0, -1).join('/') : 'Root';
	}

	// The note's containing folder. Notes live directly in the workspace, so show
	// the workspace name; only nested notes (if any) show a subfolder path.
	function folderLabel(note: NoteSummary): string {
		const segments = note.relativePath.replace(/\\/g, '/').split('/').filter(Boolean);
		if (segments.length > 1) return segments.slice(0, -1).join('/');
		return app?.workspacePath ? workspaceLabel(app.workspacePath) : 'workspace';
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

	async function createNote(extension: string = 'md', notebook: string | null = createTarget) {
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
				const note = await invoke<NoteDocument>('create_note', { title, extension, notebook });
				upsertNoteIntoLibrary(note);
			}
			await refreshApp();
		} finally { isBusy = false; createLoopRunning = false; }
	}

	function newNotebook() {
		newNotebookName = '';
		notebookDialog?.showModal();
	}

	async function confirmNewNotebook() {
		const name = newNotebookName.trim();
		if (!name) return;
		isBusy = true;
		try {
			notebooks = await invoke<string[]>('create_notebook', { name });
			activeNotebook = name;
			notebookDialog?.close();
		} catch (e) {
			console.error('create notebook failed', e);
			message = `Could not create notebook: ${e}`;
		} finally {
			isBusy = false;
		}
	}

	async function loadNotebooks() {
		try {
			notebooks = await invoke<string[]>('list_notebooks');
		} catch (e) {
			console.error('list notebooks failed', e);
		}
	}

	function toggleNotebook(nb: string) {
		activeNotebook = activeNotebook === nb ? null : nb;
	}
	function notebookCount(nb: string): number {
		return dashNotes.filter((n) => n.folder === nb || n.folder.startsWith(nb + '/')).length;
	}
	// The top-level notebook a note belongs to, or null if it's loose in the workspace.
	function notebookOf(note: NoteSummary): string | null {
		if (!note.folder || note.folder === 'Root') return null;
		return note.folder.split('/')[0];
	}
	let uncategorizedCount = $derived(dashNotes.filter((n) => notebookOf(n) === null).length);

	// Where new notes / uploads land — inferred, never asked: the open note's
	// notebook if one is selected, otherwise the notebook you're viewing. null =
	// uncategorized (workspace root).
	let createTarget = $derived(selectedNote ? notebookOf(selectedNote) : activeNotebook);

	// Upload a document (PDF/EPUB) into the workspace from the notebook "+" menu.
	async function importFile() {
		showAddMenu = false;
		const picked = await open({
			multiple: false,
			filters: [{ name: 'Documents', extensions: ['pdf', 'epub'] }]
		});
		if (typeof picked !== 'string') return;
		isBusy = true;
		try {
			await invoke('import_pdf_file', { filePath: picked, notebook: createTarget });
			await refreshApp();
		} catch (e) {
			console.error('import failed', e);
		} finally {
			isBusy = false;
		}
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

	// First click selects a row and shows its details in the right pane (closing
	// tasks); clicking the already-selected row opens it.
	function selectOrOpen(note: NoteSummary) {
		if (selectedNote?.id === note.id) {
			void openNote(note.id);
		} else {
			selectedNote = note;
			tasksCollapsed = true;
		}
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

		// Paint instantly from the last-known snapshot so coming back from a note
		// doesn't blank the UI while the backend responds.
		if (appCache.app) {
			app = appCache.app;
			provider = appCache.provider;
			appVersion = appCache.appVersion;
			ready = true;
		}

		void (async () => {
			if (!appVersion) {
				getVersion().then((v) => { appVersion = v; appCache.appVersion = v; }).catch(() => {});
			}

			// Cold start: paint the shell immediately from the cheap in-memory snapshot
			// (workspace + whatever's already indexed) so the window never sits blank.
			if (!appCache.app) {
				try {
					app = await invoke<AppSnapshot>('get_snapshot');
					provider = await invoke<ProviderStatus>('get_provider_status');
					appCache.app = app;
					appCache.provider = provider;
				} catch (e) {
					console.error(e);
				}
				ready = true;
			}

			// Heavy one-time init (git, watcher, full reindex) now runs with the UI
			// already up; the index events refresh the list when it finishes.
			try {
				if (!appCache.bootstrapped) {
					indexing = true;
					app = await invoke<AppSnapshot>('bootstrap');
					appCache.bootstrapped = true;
					appCache.app = app;
					indexing = false;
				}
				await refreshApp();
			} finally {
				ready = true;
				indexing = false;
			}
			unlistenChanged = await listen('index://changed', () => { message = 'Reindexing…'; });
			unlistenStatus = await listen<string>('index://status', (event) => {
				if (event.payload === 'started') { message = 'Indexing…'; indexing = true; }
				else if (event.payload === 'completed') { message = ''; indexing = false; void refreshApp(); }
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
<svelte:window onclick={() => { activeMenuId = null; showAddMenu = false; showNotebookMenu = false; }} />

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

<dialog bind:this={notebookDialog} class="confirm-dialog" onclose={() => { newNotebookName = ''; }}>
	<div class="dialog-content">
		<h3>New notebook</h3>
		<p>Create a notebook — a folder that holds notes of any kind.</p>
		<input
			class="nb-name-input"
			bind:value={newNotebookName}
			use:autofocus
			placeholder="Notebook name"
			onkeydown={(e) => { if (e.key === 'Enter') { e.preventDefault(); confirmNewNotebook(); } }}
		/>
		<div class="dialog-actions">
			<button class="btn-ghost" onclick={() => notebookDialog?.close()}>Cancel</button>
			<button class="btn-primary nb-create-btn" onclick={confirmNewNotebook} disabled={isBusy || !newNotebookName.trim()}>Create</button>
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
			<button
				class="theme-toggle-btn"
				onclick={toggleTheme}
				title={$theme === 'light' ? 'Switch to dark mode' : 'Switch to light mode'}
				aria-label="Toggle theme"
			>
				{#if $theme === 'light'}
					<!-- moon: click to go dark -->
					<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
						<path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z"></path>
					</svg>
				{:else}
					<!-- sun: click to go light -->
					<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
						<circle cx="12" cy="12" r="4"></circle>
						<path d="M12 2v2M12 20v2M4.93 4.93l1.41 1.41M17.66 17.66l1.41 1.41M2 12h2M20 12h2M6.34 17.66l-1.41 1.41M19.07 4.93l-1.41 1.41"></path>
					</svg>
				{/if}
			</button>
		</div>

		{#if app?.workspacePath}
			<button class="rail-search-btn" onclick={() => globalSearchDialog?.showModal()}>
				<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><circle cx="11" cy="11" r="8"/><line x1="21" y1="21" x2="16.65" y2="16.65"/></svg>
				Search notes...
			</button>
		{/if}

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
				<div class="section-label">
					<span>New</span>
				</div>
				<div class="ov-group new-actions">
					<button class="ov-row ov-clickable ov-action" onclick={() => createNote('md')} disabled={isBusy}>
						<span class="ov-key"><svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 20h9"/><path d="M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4z"/></svg>markdown</span>
					</button>
					<button class="ov-row ov-clickable ov-action" onclick={() => createNote('tex')} disabled={isBusy}>
						<span class="ov-key"><svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M18 4H6l6 8-6 8h12"/></svg>latex</span>
					</button>
					<button class="ov-row ov-clickable ov-action" onclick={() => createNote('ipynb')} disabled={isBusy}>
						<span class="ov-key"><svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="16 18 22 12 16 6"/><polyline points="8 6 2 12 8 18"/></svg>jupyter</span>
					</button>
					<button class="ov-row ov-clickable ov-action" onclick={importFile} disabled={isBusy}>
						<span class="ov-key"><svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><polyline points="17 8 12 3 7 8"/><line x1="12" y1="3" x2="12" y2="15"/></svg>upload</span>
					</button>
					<button class="new-notebook-btn" onclick={newNotebook} disabled={isBusy}>
						<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M2 3h6a4 4 0 0 1 4 4v14a3 3 0 0 0-3-3H2z"/><path d="M22 3h-6a4 4 0 0 0-4 4v14a3 3 0 0 1 3-3h7z"/></svg>
						new notebook
					</button>
				</div>

				<!-- Details panel — the page handles browsing, the rail shows workspace vitals -->
				<div class="section-label">
					<span>Library</span>
				</div>
				<div class="ov-group">
					<!-- notebooks group -->
					<button class="ov-row ov-group-head" onclick={() => (notebooksExpanded = !notebooksExpanded)}>
						<span class="ov-key">
							<svg class="ov-chevron" class:collapsed={!notebooksExpanded} width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="6 9 12 15 18 9"/></svg>
							notebooks
						</span>
						<span class="ov-val">{notebooks.length}</span>
					</button>
					{#if notebooksExpanded}
						<button class="ov-row ov-sub ov-clickable" class:active={activeNotebook === null} onclick={() => (activeNotebook = null)} title="Notes not in a notebook">
							<span class="ov-key ov-ellipsis">uncategorized</span>
							<span class="ov-val">{uncategorizedCount}</span>
						</button>
						{#each notebooks as nb (nb)}
							<button class="ov-row ov-sub ov-clickable" class:active={activeNotebook === nb} onclick={() => toggleNotebook(nb)} title={nb}>
								<span class="ov-key ov-ellipsis">{nb}</span>
								<span class="ov-val">{notebookCount(nb)}</span>
							</button>
						{/each}
					{/if}

					<!-- notes group: editable working docs -->
					<button class="ov-row ov-group-head" onclick={() => (notesExpanded = !notesExpanded)}>
						<span class="ov-key">
							<svg class="ov-chevron" class:collapsed={!notesExpanded} width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="6 9 12 15 18 9"/></svg>
							notes
						</span>
						<span class="ov-val">{typeCounts.md + typeCounts.tex + typeCounts.ipynb}</span>
					</button>
					{#if notesExpanded}
						{#each NOTE_SUBTYPES as st}
							{#if typeCounts[st.type] > 0}
								<button class="ov-row ov-sub ov-clickable" class:active={activeTypeFilter === st.type} onclick={() => setTypeFilter(activeTypeFilter === st.type ? 'all' : st.type)}>
									<span class="ov-key ov-ellipsis">{st.label}</span>
									<span class="ov-val">{typeCounts[st.type]}</span>
								</button>
							{/if}
						{/each}
					{/if}

					<!-- documents group: uploaded source material -->
					<button class="ov-row ov-group-head" onclick={() => (docsExpanded = !docsExpanded)}>
						<span class="ov-key">
							<svg class="ov-chevron" class:collapsed={!docsExpanded} width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="6 9 12 15 18 9"/></svg>
							documents
						</span>
						<span class="ov-val">{typeCounts.pdf + typeCounts.epub}</span>
					</button>
					{#if docsExpanded}
						{#each DOC_SUBTYPES as st}
							{#if typeCounts[st.type] > 0}
								<button class="ov-row ov-sub ov-clickable" class:active={activeTypeFilter === st.type} onclick={() => setTypeFilter(activeTypeFilter === st.type ? 'all' : st.type)}>
									<span class="ov-key ov-ellipsis">{st.label}</span>
									<span class="ov-val">{typeCounts[st.type]}</span>
								</button>
							{/if}
						{/each}
					{/if}

					<div class="ov-row"><span class="ov-key">tags</span><span class="ov-val">{tagCounts.length}</span></div>
					<button class="ov-row ov-clickable" onclick={openClustersDialog} title="View clusters"><span class="ov-key">clusters</span><span class="ov-val">{commonplaces.length}</span></button>
				</div>


				<div class="section-label" style="margin-top: var(--space-4);">
					<span>Tags</span>
					<span class="section-count">{tagCounts.length}</span>
				</div>
				{#if tagCounts.length > 0}
					<div class="ov-group">
						{#each tagCounts.slice(0, 12) as [tag, count] (tag)}
							<button class="ov-row ov-clickable" class:active={activeTag === tag} onclick={() => toggleTag(tag)} title="Filter by #{tag}">
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

		{#if appVersion}
			<div class="rail-version">v{appVersion}</div>
		{/if}

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
					<div class="nb-switcher">
						<button class="nb-switch-btn" onclick={(e) => { e.stopPropagation(); showNotebookMenu = !showNotebookMenu; }}>
							<h2>{activeNotebook ?? 'Uncategorized'}</h2>
							<svg class="nb-switch-caret" width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="6 9 12 15 18 9"/></svg>
						</button>
						{#if showNotebookMenu}
							<!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
							<div class="nb-switch-menu" onclick={(e) => e.stopPropagation()}>
								<button class="nb-switch-item" class:active={activeNotebook === null} onclick={() => { activeNotebook = null; showNotebookMenu = false; }}>
									<span>Uncategorized</span>
									<span class="nb-switch-count">{uncategorizedCount}</span>
								</button>
								{#each notebooks as nb (nb)}
									<button class="nb-switch-item" class:active={activeNotebook === nb} onclick={() => { activeNotebook = nb; showNotebookMenu = false; }}>
										<span class="ov-ellipsis">{nb}</span>
										<span class="nb-switch-count">{notebookCount(nb)}</span>
									</button>
								{/each}
								<div class="nb-add-divider"></div>
								<button class="nb-switch-item nb-switch-new" onclick={() => { showNotebookMenu = false; newNotebook(); }}>
									<svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><line x1="12" y1="5" x2="12" y2="19"/><line x1="5" y1="12" x2="19" y2="12"/></svg>
									New notebook
								</button>
							</div>
						{/if}
					</div>
						{#if indexing}
							<span class="indexing-pill" title="Indexing your workspace">
								<svg class="nb-spin" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round"><path d="M21 12a9 9 0 1 1-6.219-8.56"/></svg>
								indexing
							</span>
						{/if}
					<div style="display: flex; gap: var(--space-2); align-items: center;">
						<button
							class="header-toggle-btn"
							class:active={!tasksCollapsed && !selectedNote}
							onclick={() => { selectedNote = null; tasksCollapsed = !tasksCollapsed; }}
							title={tasksCollapsed ? 'Show tasks' : 'Hide tasks'}
						>
							<svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 11 12 14 22 4"/><path d="M21 12v7a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h11"/></svg>
							Tasks
							{#if dashTasks.filter((t) => !t.done).length > 0}
								<span class="header-toggle-count">{dashTasks.filter((t) => !t.done).length}</span>
							{/if}
						</button>
					</div>
				</header>

				<div class="dashboard-grid">
					<!-- Left: notebook -->
					<div class="dash-left">
						<section class="dash-section nb-section">
							<h3 class="nb-header-tabs-container">
								<button class="h3-tab" class:active={activeTypeFilter !== 'documents'} onclick={() => setTypeFilter('notes')}>
									<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M2 3h6a4 4 0 0 1 4 4v14a3 3 0 0 0-3-3H2z"/><path d="M22 3h-6a4 4 0 0 0-4 4v14a3 3 0 0 1 3-3h7z"/></svg>
									notes
								</button>
								<button class="h3-tab" class:active={activeTypeFilter === 'documents'} onclick={() => setTypeFilter('documents')}>
									<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><polyline points="14 2 14 8 20 8"/><line x1="16" y1="13" x2="8" y2="13"/><line x1="16" y1="17" x2="8" y2="17"/><polyline points="10 9 9 9 8 9"/></svg>
									documents
								</button>
								{#if activeNotebook}
									<button class="filter-chip" onclick={() => (activeNotebook = null)} title="Clear notebook filter">
										{activeNotebook}<span class="chip-x">×</span>
									</button>
								{/if}
								{#if activeTag}
									<button class="filter-chip" onclick={() => (activeTag = null)} title="Clear tag filter">
										#{activeTag}<span class="chip-x">×</span>
									</button>
								{/if}
							</h3>
							<div class="nb-list">
								{#each filteredNotebook as note (note.id)}
									<!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
									<div class="nb-row" class:selected={selectedNote?.id === note.id} onclick={() => selectOrOpen(note)} oncontextmenu={(e) => { e.preventDefault(); activeMenuId = activeMenuId === note.id ? null : note.id; }}>
										{#if pinnedNoteIds.includes(note.id)}
											<svg class="nb-pin" width="11" height="11" viewBox="0 0 24 24" fill="currentColor" stroke="none"><path d="M19 21l-7-5-7 5V5a2 2 0 0 1 2-2h10a2 2 0 0 1 2 2z"/></svg>
										{/if}
										<span class="nb-row-title">{note.title}</span>
										{#if notebookOf(note)}
											<span class="nb-row-book" title="Notebook: {notebookOf(note)}">
												<svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M2 3h6a4 4 0 0 1 4 4v14a3 3 0 0 0-3-3H2z"/><path d="M22 3h-6a4 4 0 0 0-4 4v14a3 3 0 0 1 3-3h7z"/></svg>
												{notebookOf(note)}
											</span>
										{/if}
										<span class="nb-row-date">{fullDateTime(note.createdAt)}</span>
										<span class="row-badge nb-row-badge">{getNoteBadge(note)}</span>
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
								{/each}
								{#if filteredNotebook.length === 0}
									{#if indexing}
										<div class="nb-empty nb-indexing">
											<svg class="nb-spin" width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round"><path d="M21 12a9 9 0 1 1-6.219-8.56"/></svg>
											Indexing your workspace…
										</div>
									{:else}
										<div class="nb-empty">No {activeTypeFilter === 'all' ? '' : activeTypeFilter + ' '}notes yet.</div>
									{/if}
								{/if}
							</div>
						</section>
					</div>

					<!-- Right: note details (selected row) or tasks -->
					{#if selectedNote}
						<div class="dash-right">
							<section class="dash-panel">
								<div class="panel-head">
									<h3>details</h3>
									<button class="panel-close" onclick={() => (selectedNote = null)} aria-label="Close details" title="Close">
										<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>
									</button>
								</div>
								<div class="detail-body">
									<div class="detail-head">
										<div class="detail-title">{selectedNote.title}</div>
										<span class="row-badge">{getNoteBadge(selectedNote)}</span>
									</div>
									{#if selectedNote.tags.length > 0}
										<div class="detail-tags">
											{#each selectedNote.tags as tag}<span class="kc-tag">#{tag}</span>{/each}
										</div>
									{/if}
									<div class="detail-rows">
										<div class="detail-row"><span class="dr-key">created</span><span class="dr-val">{fullDateTime(selectedNote.createdAt)}</span></div>
										<div class="detail-row"><span class="dr-key">modified</span><span class="dr-val">{fullDateTime(selectedNote.updatedAt)}</span></div>
										<div class="detail-row"><span class="dr-key">folder</span><span class="dr-val">{folderLabel(selectedNote)}</span></div>
										<div class="detail-row"><span class="dr-key">path</span><span class="dr-val" title={selectedNote.relativePath}>{selectedNote.relativePath}</span></div>
										<div class="detail-row"><span class="dr-key">links</span><span class="dr-val">{selectedNote.backlinks.length}</span></div>
									</div>
									{#if selectedNote.excerpt}
										<p class="detail-excerpt">{selectedNote.excerpt}</p>
									{/if}
									<button class="btn-primary detail-open" onclick={() => selectedNote && openNote(selectedNote.id)}>Open</button>
								</div>
							</section>
						</div>
					{:else if !tasksCollapsed}
						<div class="dash-right">
							<section class="dash-panel">
								<div class="panel-head">
									<h3>tasks</h3>
									<div class="panel-tabs">
										<button class:active={activeTaskFilter === 'all'} onclick={() => (activeTaskFilter = 'all')}>all</button>
										<button class:active={activeTaskFilter === 'active'} onclick={() => (activeTaskFilter = 'active')}>active</button>
										<button class:active={activeTaskFilter === 'done'} onclick={() => (activeTaskFilter = 'done')}>done</button>
									</div>
								</div>
								<div class="task-list">
									{#each filteredTasks as task (task.id)}
										<label class="task-item" class:done={task.done}>
											<input class="task-check" type="checkbox" bind:checked={task.done} />
											<span class="task-text">{task.text}</span>
											<button class="task-remove" onclick={(e) => { e.preventDefault(); removeTask(task.id); }} aria-label="Remove task">&times;</button>
										</label>
									{/each}
									{#if filteredTasks.length === 0}
										<div class="nb-empty">No {activeTaskFilter === 'all' ? '' : activeTaskFilter + ' '}tasks.</div>
									{/if}
								</div>
								<form class="add-task-form" onsubmit={(e) => { e.preventDefault(); addTask(); }}>
									<input type="text" placeholder="Add a task..." bind:value={newTaskText} />
									<button type="submit" class="btn-primary" style="padding: 6px 14px; border-radius: var(--radius-xs); font-size: 0.8rem; font-weight: 500; min-height: unset; line-height: 1;">Add</button>
								</form>
							</section>
						</div>
					{/if}
				</div>
			</div>
		{/if}
	</main>
</div>

{#if isClustersListOpen}
	<!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
	<div class="modal-overlay" onclick={closeClustersList}>
		<div class="modal-content" onclick={(e) => e.stopPropagation()}>
			<header class="modal-header">
				<h2>Clusters ({commonplaces.length})</h2>
				<p class="modal-subtitle">Groups of notes connected by links. Select one to view its notes.</p>
			</header>
			<div class="modal-body">
				{#if commonplaces.length === 0}
					<p class="cluster-empty">No clusters yet. Link notes together to form connections.</p>
				{:else}
					<div class="cluster-list">
						{#each commonplaces as cluster, i}
							<button class="cluster-row" onclick={() => { closeClustersList(); openCluster(cluster); }}>
								<span class="cluster-name">
									<svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="3"/><circle cx="5" cy="6" r="2"/><circle cx="19" cy="6" r="2"/><circle cx="6" cy="19" r="2"/><path d="M9.5 10.5 6.7 7.3M14.5 10.5l2.8-3.2M10.2 14.2 7.4 17.4"/></svg>
									Cluster {i + 1}
								</span>
								<span class="cluster-meta">{cluster.length} notes</span>
							</button>
						{/each}
					</div>
				{/if}
			</div>
			<footer class="modal-footer">
				<button class="btn-cancel" onclick={closeClustersList}>Close</button>
			</footer>
		</div>
	</div>
{/if}

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
			box-shadow: 4px 0 24px var(--shadow-color-strong);
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

	.theme-toggle-btn {
		display: flex;
		align-items: center;
		justify-content: center;
		width: 30px;
		height: 30px;
		flex-shrink: 0;
		background: transparent;
		border: 1px solid transparent;
		border-radius: var(--radius-sm);
		color: var(--text-secondary);
		cursor: pointer;
		transition: color 0.15s, background 0.15s, border-color 0.15s;
	}
	.theme-toggle-btn:hover {
		color: var(--accent-100);
		background: var(--hover-overlay);
		border-color: var(--border-default);
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

	.rail-search-btn {
		display: flex;
		align-items: center;
		gap: 8px;
		margin: 0 var(--space-4) var(--space-3);
		padding: 8px 12px;
		background: var(--bg-input);
		border: 1px solid var(--border-default);
		border-radius: var(--radius-sm);
		color: var(--text-secondary);
		font-family: var(--font-mono);
		font-size: 0.85rem;
		cursor: pointer;
		flex-shrink: 0;
		transition: border-color 0.15s, color 0.15s;
	}
	.rail-search-btn:hover {
		border-color: var(--neutral-600);
		color: var(--text-primary);
	}

	.rail-list {
		flex: 1;
		overflow-y: auto;
		padding: 0 var(--space-3) var(--space-4);
		scrollbar-width: none;
	}
	.rail-list::-webkit-scrollbar { display: none; }

	.rail-empty {
		font-size: 0.8rem;
		color: var(--text-secondary);
		padding: var(--space-3);
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
		color: var(--text-hero);
		user-select: none;
	}
	.section-count {
		font-size: 0.82rem;
		color: var(--text-hero);
		font-weight: 400;
	}
	.section-add {
		display: flex;
		align-items: center;
		justify-content: center;
		width: 18px;
		height: 18px;
		padding: 0;
		background: transparent;
		border: none;
		border-radius: var(--radius-xs);
		color: var(--text-secondary);
		cursor: pointer;
		transition: color 0.15s, background 0.15s;
	}
	.section-add:hover:not(:disabled) {
		color: var(--accent-100);
		background: var(--hover-overlay);
	}
	.section-add:disabled {
		opacity: 0.4;
		cursor: not-allowed;
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
		background: var(--hover-overlay);
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
		background: var(--overlay-faint);
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
	.row-menu-btn:hover { background: var(--hover-overlay-strong); color: var(--text-primary); }

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
		box-shadow: 0 4px 12px var(--shadow-color);
	}
	.row-delete {
		width: 100%;
		text-align: left;
		padding: var(--space-2) var(--space-3);
		font-size: 0.9rem;
		font-family: var(--font-mono);
		background: transparent;
		color: var(--danger);
		border: none;
		border-radius: var(--radius-xs);
		cursor: pointer;
	}
	.row-delete:hover { background: var(--danger-tint); }

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
		padding: 6px 0;
		font-size: 0.8rem;
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
		font-size: 0.78rem;
	}
	.ov-val.ov-ok { color: var(--success); }
	.ov-clickable { cursor: pointer; }
	.ov-clickable:hover .ov-key { color: var(--text-primary); }
	/* Create / upload action rows in the "New" section */
	.ov-action {
		border-bottom: none;
		border-radius: var(--radius-xs);
		padding: 6px var(--space-2);
	}
	/* "New" actions laid out as a 2×2 grid */
	.new-actions {
		display: grid;
		grid-template-columns: 1fr 1fr;
		gap: 2px;
	}
	.new-actions .ov-action {
		width: auto;
		justify-content: flex-start;
		border: 1px solid var(--border-subtle);
	}
	.new-notebook-btn {
		grid-column: 1 / -1;
		display: flex;
		align-items: center;
		justify-content: center;
		gap: 8px;
		margin-top: 2px;
		padding: 7px;
		background: transparent;
		border: 1px solid var(--border-subtle);
		border-radius: var(--radius-xs);
		color: var(--text-primary);
		font-family: var(--font-mono);
		font-size: 0.8rem;
		cursor: pointer;
		transition: border-color 0.15s, background 0.15s, color 0.15s;
	}
	.new-notebook-btn svg {
		color: var(--accent-200);
	}
	.new-notebook-btn:hover:not(:disabled) {
		border-color: var(--accent-300);
		background: var(--hover-overlay);
		color: var(--accent-100);
	}
	.new-notebook-btn:disabled {
		opacity: 0.5;
		cursor: not-allowed;
	}
	.ov-action .ov-key svg { color: var(--accent-200); }
	.ov-action:hover:not(:disabled) { background: var(--hover-overlay); }
	.ov-action:disabled { opacity: 0.5; cursor: not-allowed; }
	.ov-row.active .ov-key,
	.ov-row.active .ov-val {
		color: var(--accent-100);
	}
	.ov-row.active .ov-key svg {
		color: var(--accent-100);
	}
	/* Collapsible group heads (notes / documents) + their indented sub-rows. */
	.ov-group-head {
		cursor: pointer;
	}
	.ov-group-head .ov-key {
		color: var(--text-primary);
		font-weight: 500;
	}
	.ov-group-head:hover .ov-key {
		color: var(--accent-100);
	}
	.ov-chevron {
		transition: transform 0.15s ease;
	}
	.ov-chevron.collapsed {
		transform: rotate(-90deg);
	}
	.ov-row.ov-sub {
		padding-left: 20px;
		font-size: 0.78rem;
	}
	.ov-row.ov-sub .ov-key {
		color: var(--text-secondary);
	}

	.rail-version {
		flex-shrink: 0;
		padding: 0 var(--space-4) var(--space-2);
		font-size: 0.7rem;
		color: var(--text-secondary);
		opacity: 0.6;
		font-family: var(--font-mono);
	}
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
		background: var(--hover-overlay);
	}
	.footer-change-btn:disabled { opacity: 0.4; cursor: not-allowed; }
	.footer-dot {
		width: 6px;
		height: 6px;
		border-radius: 50%;
		background: var(--neutral-700);
		flex-shrink: 0;
	}
	.footer-dot.dot-ok { background: var(--success); }

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
		color: var(--on-accent);
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
		box-shadow: 0 8px 32px var(--shadow-color) !important;
	}
	.confirm-dialog::backdrop {
		background: var(--scrim-soft) !important;
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
	.nb-name-input {
		width: 100%;
		box-sizing: border-box;
		background: var(--bg-page);
		border: 1px solid var(--border-default);
		border-radius: var(--radius-sm);
		color: var(--text-primary);
		font-family: var(--font-mono);
		font-size: 0.9rem;
		padding: 9px 11px;
		outline: none;
		transition: border-color 0.15s;
	}
	.nb-name-input:focus { border-color: var(--accent-200); }
	.nb-create-btn {
		padding: 6px 14px;
		font-size: 0.75rem;
		min-height: unset;
		line-height: 1;
		border-radius: var(--radius-sm);
	}
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
		border: 1px solid var(--danger);
		border-radius: var(--radius-sm);
		color: var(--danger);
		cursor: pointer;
	}
	.btn-danger:hover:not(:disabled) { background: var(--danger-tint); }
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
		overflow: hidden;
		color: var(--text-primary);
	}

	.dashboard-header {
		flex-shrink: 0;
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

	/* Notebook switcher (replaces the static workspace title) */
	.nb-switcher {
		position: relative;
	}
	.nb-switch-btn {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		background: transparent;
		border: none;
		padding: 0;
		cursor: pointer;
		max-width: 100%;
	}
	.nb-switch-btn h2 {
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}
	.nb-switch-caret {
		flex-shrink: 0;
		color: var(--text-secondary);
		margin-top: 6px;
		transition: color 0.15s;
	}
	.nb-switch-btn:hover .nb-switch-caret {
		color: var(--text-primary);
	}
	.nb-switch-menu {
		position: absolute;
		top: 100%;
		left: 0;
		margin-top: 6px;
		z-index: 50;
		min-width: 230px;
		max-width: 320px;
		background: var(--bg-panel);
		border: 1px solid var(--border-default);
		border-radius: var(--radius-md);
		padding: var(--space-1);
		box-shadow: 0 8px 24px var(--shadow-color);
		display: flex;
		flex-direction: column;
		gap: 1px;
	}
	.nb-switch-item {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 8px;
		width: 100%;
		text-align: left;
		background: transparent;
		border: none;
		border-radius: var(--radius-xs);
		color: var(--text-primary);
		font-family: var(--font-mono);
		font-size: 0.85rem;
		padding: 7px 10px;
		cursor: pointer;
		transition: background 0.1s;
	}
	.nb-switch-item:hover {
		background: var(--hover-overlay);
	}
	.nb-switch-item.active {
		color: var(--accent-100);
	}
	.nb-switch-count {
		flex-shrink: 0;
		color: var(--text-secondary);
		font-size: 0.75rem;
	}
	.nb-switch-check {
		flex-shrink: 0;
		color: var(--accent-100);
	}
	.nb-switch-new {
		justify-content: flex-start;
		gap: 8px;
		color: var(--text-secondary);
	}
	.nb-switch-new svg {
		color: var(--accent-200);
	}
	.nb-switch-new:hover {
		color: var(--text-primary);
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
		background: var(--hover-overlay-strong);
		border-color: var(--border-default);
	}

	.dashboard-grid {
		display: flex;
		gap: 2.5rem;
		align-items: stretch;
		flex: 1;
		min-height: 0;
	}
	.dash-left {
		flex: 1;
		min-width: 0;
		display: flex;
		flex-direction: column;
		min-height: 0;
	}
	.dash-right {
		width: 400px;
		flex-shrink: 0;
		display: flex;
		flex-direction: column;
		/* Hug content at the top instead of stretching to the notes column height,
		   so the tasks panel's height is independent of the notes list. */
		align-self: flex-start;
	}

	/* The notes list and the tasks list each scroll on their own. */
	.nb-section {
		flex: 1;
		min-height: 0;
		display: flex;
		flex-direction: column;
	}
	.nb-section > h3 {
		flex-shrink: 0;
	}

	/* ── Notebook (left) ── */
	.dash-section h3.nb-header-tabs-container {
		padding-bottom: 0;
		text-transform: none;
		letter-spacing: normal;
	}
	.h3-tab {
		display: flex;
		align-items: center;
		gap: 6px;
		background: transparent;
		border: none;
		color: var(--text-secondary);
		cursor: pointer;
		font-size: 0.85rem;
		font-family: var(--font-mono);
		padding: 0 4px 8px 4px;
		margin-bottom: -1px;
		border-bottom: 2px solid transparent;
		transition: color 0.15s, border-color 0.15s;
	}
	.h3-tab:hover {
		color: var(--text-primary);
	}
	.h3-tab.active {
		color: var(--text-primary);
		border-bottom-color: var(--text-primary);
	}
	.h3-tab svg {
		color: var(--neutral-600);
	}

	.nb-list {
		display: flex;
		flex-direction: column;
		flex: 1;
		min-height: 0;
		overflow-y: auto;
	}
	.nb-list::-webkit-scrollbar,
	.task-list::-webkit-scrollbar {
		width: 6px;
	}
	.nb-list::-webkit-scrollbar-thumb,
	.task-list::-webkit-scrollbar-thumb {
		background: var(--border-default);
		border-radius: 4px;
	}
	.nb-row {
		display: flex;
		align-items: center;
		gap: var(--space-3);
		padding: 9px var(--space-2);
		border-bottom: 1px solid var(--border-subtle);
		cursor: pointer;
		position: relative;
		transition: background 0.1s;
	}
	.nb-row:hover {
		background: var(--hover-overlay);
	}
	.nb-row.selected {
		background: var(--accent-tint);
		box-shadow: inset 2px 0 0 var(--accent-200);
	}
	.nb-row:last-child {
		border-bottom: none;
	}
	.nb-row:hover .row-menu-btn {
		opacity: 1;
	}
	.nb-pin {
		color: var(--accent-200);
		flex-shrink: 0;
	}
	.nb-row-title {
		flex: 1;
		min-width: 0;
		font-size: 0.95rem;
		color: var(--text-primary);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}
	.nb-row-date {
		flex-shrink: 0;
		font-size: 0.78rem;
		color: var(--text-secondary);
		font-family: var(--font-mono);
	}
	.nb-row-book {
		display: inline-flex;
		align-items: center;
		gap: 4px;
		flex-shrink: 0;
		max-width: 9rem;
		overflow: hidden;
		white-space: nowrap;
		text-overflow: ellipsis;
		font-size: 0.72rem;
		font-family: var(--font-mono);
		color: var(--text-secondary);
		background: var(--overlay-faint);
		border: 1px solid var(--border-subtle);
		border-radius: 999px;
		padding: 1px 8px;
	}
	.nb-row-book svg {
		flex-shrink: 0;
		color: var(--accent-200);
	}
	.nb-row-badge {
		flex-shrink: 0;
	}
	.nb-empty {
		font-size: 0.85rem;
		color: var(--neutral-600);
		padding: var(--space-4) var(--space-2);
	}
	.nb-indexing {
		display: flex;
		align-items: center;
		gap: 8px;
		color: var(--text-secondary);
	}
	.nb-spin {
		animation: nb-spin 0.8s linear infinite;
	}
	@keyframes nb-spin {
		to { transform: rotate(360deg); }
	}
	.indexing-pill {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		font-family: var(--font-mono);
		font-size: 0.72rem;
		color: var(--accent-100);
		background: var(--accent-tint);
		border: 1px solid var(--accent-300);
		border-radius: 999px;
		padding: 2px 10px;
		white-space: nowrap;
	}
	/* ── Right panel (tasks) ── */
	.dash-panel {
		border: 1px solid var(--border-default);
		border-radius: var(--radius-lg);
		background: var(--bg-panel);
		padding: var(--space-4);
		display: flex;
		flex-direction: column;
	}
	.panel-head {
		display: flex;
		align-items: center;
		justify-content: space-between;
		margin-bottom: var(--space-3);
		flex-shrink: 0;
	}
	.panel-head h3 {
		margin: 0;
		font-size: 0.8rem;
		font-weight: 700;
		text-transform: uppercase;
		letter-spacing: 0.08em;
		color: var(--text-hero);
	}
	.panel-close {
		display: flex;
		align-items: center;
		justify-content: center;
		background: transparent;
		border: none;
		color: var(--text-secondary);
		cursor: pointer;
		padding: 2px;
		border-radius: var(--radius-xs);
	}
	.panel-close:hover {
		color: var(--text-primary);
		background: var(--hover-overlay);
	}

	/* ── Note details pane ── */
	.detail-body {
		display: flex;
		flex-direction: column;
		gap: var(--space-3);
		overflow-y: auto;
	}
	.detail-head {
		display: flex;
		align-items: flex-start;
		justify-content: space-between;
		gap: var(--space-2);
	}
	.detail-title {
		font-size: 1.05rem;
		font-weight: 600;
		color: var(--text-primary);
		line-height: 1.3;
		word-break: break-word;
	}
	.detail-tags {
		display: flex;
		flex-wrap: wrap;
		gap: 4px;
	}
	.detail-rows {
		display: flex;
		flex-direction: column;
	}
	.detail-row {
		display: flex;
		align-items: baseline;
		justify-content: space-between;
		gap: var(--space-3);
		padding: 6px 0;
		border-bottom: 1px solid var(--border-subtle);
		font-family: var(--font-mono);
		font-size: 0.78rem;
	}
	.detail-row:last-child {
		border-bottom: none;
	}
	.dr-key {
		color: var(--text-secondary);
		flex-shrink: 0;
	}
	.dr-val {
		color: var(--text-primary);
		text-align: right;
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.detail-excerpt {
		margin: 0;
		font-size: 0.82rem;
		line-height: 1.5;
		color: var(--text-secondary);
		display: -webkit-box;
		-webkit-line-clamp: 5;
		-webkit-box-orient: vertical;
		overflow: hidden;
	}
	.detail-open {
		margin-top: var(--space-1);
		padding: 8px 14px;
		border-radius: var(--radius-sm);
		font-size: 0.85rem;
		font-weight: 500;
		text-align: center;
	}
	.panel-tabs {
		display: flex;
		gap: 2px;
	}
	.panel-tabs button {
		background: transparent;
		border: none;
		color: var(--text-secondary);
		font-family: var(--font-mono);
		font-size: 0.72rem;
		padding: 2px 8px;
		border-radius: 999px;
		cursor: pointer;
		transition: color 0.15s, background 0.15s;
	}
	.panel-tabs button:hover {
		color: var(--text-primary);
	}
	.panel-tabs button.active {
		color: var(--accent-100);
		background: var(--accent-tint);
	}

	/* "+" add control at the end of the tabs row */
	.nb-add-wrap {
		margin-left: auto;
		position: relative;
	}
	.nb-add-btn {
		display: flex;
		align-items: center;
		justify-content: center;
		width: 28px;
		height: 28px;
		background: transparent;
		border: 1px solid var(--border-default);
		border-radius: var(--radius-sm);
		color: var(--text-secondary);
		cursor: pointer;
		transition: color 0.15s, background 0.15s, border-color 0.15s;
	}
	.nb-add-btn:hover:not(:disabled) {
		color: var(--accent-100);
		border-color: var(--accent-300);
		background: var(--hover-overlay);
	}
	.nb-add-btn:disabled {
		opacity: 0.5;
		cursor: not-allowed;
	}
	.nb-add-menu {
		position: absolute;
		top: calc(100% + 6px);
		right: 0;
		z-index: 30;
		min-width: 170px;
		background: var(--bg-panel);
		border: 1px solid var(--border-default);
		border-radius: var(--radius-md);
		padding: var(--space-1);
		box-shadow: 0 8px 24px var(--shadow-color);
		display: flex;
		flex-direction: column;
		gap: 1px;
	}
	.nb-add-item {
		display: flex;
		align-items: center;
		gap: 8px;
		width: 100%;
		text-align: left;
		background: transparent;
		border: none;
		border-radius: var(--radius-xs);
		color: var(--text-primary);
		font-family: var(--font-mono);
		font-size: 0.85rem;
		padding: 7px 10px;
		cursor: pointer;
		transition: background 0.1s;
	}
	.nb-add-item:hover {
		background: var(--hover-overlay);
	}
	.nb-add-item .plus {
		color: var(--accent-100);
		font-weight: 700;
		font-size: 1rem;
		line-height: 1;
		width: 13px;
		text-align: center;
	}
	.nb-add-item svg {
		color: var(--text-secondary);
		flex-shrink: 0;
	}
	.nb-add-divider {
		height: 1px;
		background: var(--border-subtle);
		margin: var(--space-1) 4px;
	}

	.task-list {
		display: flex;
		flex-direction: column;
		max-height: 60vh;
		overflow-y: auto;
		margin-bottom: var(--space-3);
	}
	.task-item {
		display: flex;
		align-items: flex-start;
		gap: 10px;
		padding: 10px 4px;
		border-bottom: 1px solid var(--border-subtle);
		cursor: pointer;
		position: relative;
		transition: background 0.1s;
	}
	.task-item:last-child {
		border-bottom: none;
	}
	.task-item:hover {
		background: var(--hover-overlay);
	}
	.task-check {
		margin-top: 1px;
		width: 15px;
		height: 15px;
		flex-shrink: 0;
		accent-color: var(--accent-300);
		cursor: pointer;
	}
	.task-text {
		flex: 1;
		min-width: 0;
		font-size: 0.85rem;
		color: var(--text-primary);
		line-height: 1.5;
		word-break: break-word;
		padding-right: 14px;
	}
	.task-item.done .task-text {
		text-decoration: line-through;
		color: var(--neutral-600);
	}
	.task-remove {
		position: absolute;
		right: 2px;
		top: 9px;
		background: transparent;
		border: none;
		color: var(--text-secondary);
		cursor: pointer;
		font-size: 1.2rem;
		line-height: 1;
		opacity: 0;
		transition: opacity 0.1s;
	}
	.task-item:hover .task-remove {
		opacity: 1;
	}
	.task-remove:hover {
		color: var(--danger);
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
		color: var(--text-hero);
		margin: 0 0 0.5rem 0;
		border-bottom: 1px solid var(--border-default);
		padding-bottom: 6px;
	}
	.dash-section h3 svg { color: var(--neutral-600); }

	/* Active sidebar filter shown in the notes header; click to clear. */
	.filter-chip {
		display: inline-flex;
		align-items: center;
		gap: 5px;
		text-transform: none;
		letter-spacing: 0;
		font-family: var(--font-mono);
		font-size: 0.72rem;
		font-weight: 500;
		color: var(--accent-100);
		background: var(--accent-tint);
		border: 1px solid var(--accent-300);
		border-radius: 999px;
		padding: 2px 8px;
		cursor: pointer;
	}
	.filter-chip .chip-x {
		font-size: 0.9rem;
		line-height: 1;
		opacity: 0.8;
	}
	.filter-chip:hover .chip-x { opacity: 1; }

	.add-task-form {
		margin-top: 0;
		display: flex;
		gap: 8px;
		flex-shrink: 0;
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


	/* ── Clusters Modal ── */
	.modal-overlay {
		position: fixed;
		top: 0; left: 0; right: 0; bottom: 0;
		background: var(--scrim);
		backdrop-filter: blur(2px);
		display: flex;
		align-items: center;
		justify-content: center;
		z-index: 1000;
		animation: fade-in 0.2s ease-out;
	}
	.modal-content {
		background: var(--bg-modal);
		border: 1px solid var(--border-default);
		border-radius: var(--radius-sm);
		width: 90%;
		max-width: 700px;
		max-height: 80vh;
		display: flex;
		flex-direction: column;
		box-shadow: 0 15px 40px var(--shadow-color-strong);
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

	/* Clusters list dialog */
	.cluster-list {
		display: flex;
		flex-direction: column;
		padding: var(--space-2) 0;
	}
	.cluster-row {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--space-3);
		width: 100%;
		background: transparent;
		border: none;
		border-bottom: 1px solid var(--border-subtle);
		padding: 0.85rem 1.5rem;
		cursor: pointer;
		text-align: left;
		font-family: var(--font-mono);
		transition: background 0.1s;
	}
	.cluster-row:last-child { border-bottom: none; }
	.cluster-row:hover { background: var(--hover-overlay); }
	.cluster-name {
		display: flex;
		align-items: center;
		gap: 8px;
		font-size: 0.9rem;
		color: var(--text-primary);
	}
	.cluster-name svg { color: var(--accent-200); flex-shrink: 0; }
	.cluster-meta {
		flex-shrink: 0;
		font-size: 0.8rem;
		color: var(--text-secondary);
	}
	.cluster-empty {
		padding: 1.5rem;
		margin: 0;
		font-size: 0.9rem;
		color: var(--text-secondary);
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
		background: var(--hover-overlay);
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
		background: var(--hover-overlay);
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
		background: var(--overlay-faint);
		flex-shrink: 0;
		width: 80px;
		text-align: center;
		box-sizing: border-box;
	}

	.contents-single-list .table-row:last-child {
		border-bottom: none;
	}
	.table-row:hover {
		background: var(--overlay-faint);
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

	.header-search-btn {
		display: flex;
		align-items: center;
		gap: 8px;
		background: var(--bg-input);
		border: 1px solid var(--border-default);
		border-radius: var(--radius-sm);
		padding: 8px 12px;
		color: var(--text-secondary);
		font-family: var(--font-mono);
		font-size: 0.85rem;
		cursor: pointer;
		width: 400px;
		box-sizing: border-box;
		transition: border-color 0.15s, color 0.15s;
	}
	.header-search-btn:hover {
		border-color: var(--neutral-600);
		color: var(--text-primary);
	}

	/* Tasks show/hide toggle, sits to the right of the search bar. */
	.header-toggle-btn {
		display: flex;
		align-items: center;
		gap: 6px;
		background: var(--bg-input);
		border: 1px solid var(--border-default);
		border-radius: var(--radius-sm);
		padding: 8px 12px;
		color: var(--text-secondary);
		font-family: var(--font-mono);
		font-size: 0.85rem;
		cursor: pointer;
		transition: border-color 0.15s, color 0.15s, background 0.15s;
	}
	.header-toggle-btn:hover {
		border-color: var(--neutral-600);
		color: var(--text-primary);
	}
	.header-toggle-btn.active {
		color: var(--accent-100);
		border-color: var(--accent-300);
	}
	.header-toggle-count {
		font-size: 0.72rem;
		min-width: 16px;
		text-align: center;
		color: var(--on-accent);
		background: var(--accent-200);
		border-radius: 999px;
		padding: 0 5px;
		line-height: 16px;
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
		background: var(--scrim);
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
		background: var(--accent-tint);
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
