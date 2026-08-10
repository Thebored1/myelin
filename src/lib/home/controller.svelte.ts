import { invoke } from '@tauri-apps/api/core';
import { goto } from '$app/navigation';
import { resolve } from '$app/paths';
import { open } from '@tauri-apps/plugin-dialog';
import { appCache } from '$lib/appCache';
import type { AppSnapshot, NoteDocument, NoteSummary, ProviderStatus, SearchResponse, StorageIssue } from '$lib/types';
import { NOTE_GROUP, DOCUMENT_GROUP as DOC_GROUP, noteType } from '$lib/home/model';
import { createHomeLifecycle } from './lifecycle.svelte';
import { createTaskController } from '$lib/tasks/controller.svelte';
import type { TaskItem } from '$lib/tasks/types';
import { readBrowserStorage } from './storage';

export function createHomeController() {
	let appVersion = $state('');
	let app = $state<AppSnapshot | null>(null);
	// True once the initial snapshot has loaded — prevents the "no workspace"
	// welcome screen from flashing before we know if a workspace is connected.
	let ready = $state(false);
	let indexing = $state(false);
	let provider = $state<ProviderStatus | null>(null);
	let embeddingModelPath = $state<string | null>(null);
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

	let dashTasks = $state<TaskItem[]>([]);
	let expandedTaskId = $state<string | number | null>(null);
	let currentWorkspaceForTasks = $state<string | null>(null);
	let tasksLoadedWorkspace = $state<string | null>(null);
	const taskController = createTaskController();
	let pinnedNoteIds = $state<string[]>([]);
	let showTimeline = $state(true);
	let tasksCollapsed = $state(false);
	let browserStorageIssues = $state<StorageIssue[]>([]);

	function reportBrowserStorageIssue(issueOrKey: StorageIssue | string, error: unknown) {
		const issue: StorageIssue = typeof issueOrKey === 'string' ? {
			code: 'browser-storage', severity: 'warning', path: issueOrKey,
			message: 'A browser preference could not be saved; the rest of the library remains available.',
			recoverable: true
		} : issueOrKey;
		if (!browserStorageIssues.some((existing) => existing.path === issue.path)) {
			browserStorageIssues = [...browserStorageIssues, issue];
		}
		console.warn(`Could not persist browser preference ${issue.path}`, error);
	}

	$effect(() => {
		if (app?.workspacePath && app.workspacePath !== currentWorkspaceForTasks) {
			currentWorkspaceForTasks = app.workspacePath;
			browserStorageIssues = [];
			tasksLoadedWorkspace = null;
			activeTag = null;
			selectedNote = null;
			activeNotebook = null;
			void taskController
				.load(app.workspacePath)
				.then((tasks) => {
					if (currentWorkspaceForTasks === app?.workspacePath) {
						dashTasks = tasks;
						tasksLoadedWorkspace = app.workspacePath;
					}
				})
				.catch((error) => {
					message = String(error);
					dashTasks = [];
				});
			const pinnedKey = `pinned_${app.workspacePath}`;
			const storedPinned = readBrowserStorage(pinnedKey, reportBrowserStorageIssue);
			if (storedPinned) {
				try {
					pinnedNoteIds = JSON.parse(storedPinned);
				} catch {
					pinnedNoteIds = [];
				}
			} else {
				pinnedNoteIds = [];
			}
			const timelineKey = `timeline_${app.workspacePath}`;
			const storedTimeline = readBrowserStorage(timelineKey, reportBrowserStorageIssue);
			if (storedTimeline !== null) {
				showTimeline = storedTimeline === 'true';
			} else {
				showTimeline = true;
			}
			tasksCollapsed = readBrowserStorage(`taskscollapsed_${app.workspacePath}`, reportBrowserStorageIssue) === 'true';
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
		if (currentWorkspaceForTasks && tasksLoadedWorkspace === currentWorkspaceForTasks) {
			taskController.scheduleSave(currentWorkspaceForTasks, dashTasks);
			const writes = [
				[`pinned_${currentWorkspaceForTasks}`, JSON.stringify(pinnedNoteIds)],
				[`timeline_${currentWorkspaceForTasks}`, showTimeline.toString()],
				[`taskscollapsed_${currentWorkspaceForTasks}`, tasksCollapsed.toString()]
			] as const;
			for (const [key, value] of writes) {
				try {
					localStorage.setItem(key, value);
				} catch (error) {
					reportBrowserStorageIssue(key, error);
				}
			}
		}
	});

	let newTaskText = $state('');

	let isClusterDialogOpen = $state(false);
	let selectedCluster = $state<NoteSummary[]>([]);

	function togglePin(id: string) {
		if (pinnedNoteIds.includes(id)) {
			pinnedNoteIds = pinnedNoteIds.filter((pid) => pid !== id);
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
	async function removeTask(id: string | number) {
		const previous = dashTasks;
		dashTasks = dashTasks.filter((t) => t.id !== id);
		if (!currentWorkspaceForTasks) return;
		const removed = await taskController.remove(currentWorkspaceForTasks, id);
		if (!removed) {
			dashTasks = previous;
			dashTasks = await taskController.load(currentWorkspaceForTasks).catch(() => previous);
		}
	}

	let visibleNotes = $derived.by(() => {
		const base = (
			query && searchResults ? searchResults.results.map((r) => r.note) : (app?.notes ?? [])
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
	// The main notebook view is notes-first. Standalone source documents remain
	// available through the explicit documents tab and sidebar filter.
	let activeTypeFilter = $state<NbFilter>('notes');


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
	// Attachment copies are documents created by clone_pdf_for_attachment with a
	// generated name ("stem 1.pdf", "stem 2.pdf", … or a uuid fallback) and
	// referenced by a working note's sourcePdf. They are hidden from the
	// documents group so only standalone originals (or unattached imports)
	// appear there. Originals referenced by legacy attachments stay listed.
	let attachedDocIds = $derived.by(() => {
		const referenced = new Set(
			(app?.notes ?? []).map((n) => n.sourcePdf).filter((id): id is string => !!id)
		);
		const isCopyName = (n: NoteSummary) => {
			const name = n.relativePath.split(/[\\/]/).pop()?.toLowerCase() ?? '';
			return (
				/ \d+\.(pdf|epub)$/.test(name) ||
				/ [0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\.(pdf|epub)$/.test(name)
			);
		};
		return new Set(
			(app?.notes ?? []).filter((n) => referenced.has(n.id) && isCopyName(n)).map((n) => n.id)
		);
	});
	let typeCounts = $derived.by(() => {
		const c: Record<string, number> = { md: 0, tex: 0, ipynb: 0, pdf: 0, epub: 0 };
		for (const n of dashNotes) {
			if (DOC_GROUP.includes(noteType(n)) && attachedDocIds.has(n.id)) continue;
			c[noteType(n)] += 1;
		}
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
		else if (f === 'documents')
			base = base.filter((n) => DOC_GROUP.includes(noteType(n)) && !attachedDocIds.has(n.id));
		else if (f !== 'all') base = base.filter((n) => noteType(n) === f && !attachedDocIds.has(n.id));
		if (activeTag !== null) base = base.filter((n) => n.tags.includes(activeTag!));
		// null notebook = "uncategorized" (notes not in any notebook / workspace root).
		if (activeNotebook === null) base = base.filter((n) => notebookOf(n) === null);
		else
			base = base.filter(
				(n) => n.folder === activeNotebook || n.folder.startsWith(activeNotebook + '/')
			);
		return [...base].sort(
			(a, b) => (pinnedNoteIds.includes(b.id) ? 1 : 0) - (pinnedNoteIds.includes(a.id) ? 1 : 0)
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
						if (!visited.has(neighbor)) {
							visited.add(neighbor);
							queue.push(neighbor);
						}
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
		appCache.app = app;
		// Provider inspection may wait on a cold model-server startup. It is useful
		// status information, but should never delay the library or note shell.
		provider = app.providerStatus;
		void invoke<ProviderStatus>('get_provider_status')
			.then((status) => {
				provider = status;
			})
			.catch(() => {});
		void invoke<string | null>('get_embed_model_path')
			.then((path) => {
				embeddingModelPath = path;
			})
			.catch(() => {});
		if (query.trim()) {
			searchResults = await invoke<SearchResponse>('search_notes', { query });
		} else {
			searchResults = null;
		}
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
			id: note.id,
			title: note.title,
			tags: note.tags,
			folder: folderFromRelativePath(note.relativePath),
			excerpt: excerptFromBody(note.body),
			relativePath: note.relativePath,
			createdAt: note.createdAt,
			updatedAt: note.updatedAt,
			backlinks: note.backlinks
		};
		const existingIndex = app.notes.findIndex((e) => e.id === note.id);
		if (existingIndex >= 0) {
			app.notes[existingIndex] = summary;
		} else {
			app.notes = [summary, ...app.notes];
		}
		if (!app.customNoteOrder.includes(note.id))
			app.customNoteOrder = [...app.customNoteOrder, note.id];
		if (!app.libraryFacets.folders.includes(summary.folder))
			app.libraryFacets.folders = [...app.libraryFacets.folders, summary.folder].sort();
		const mergedTags = new Set([...app.libraryFacets.tags, ...summary.tags]);
		app.libraryFacets.tags = [...mergedTags].sort();
	}

	async function pickWorkspace() {
		const picked = await open({
			directory: true,
			multiple: false,
			title: 'Choose your markdown workspace'
		});
		if (typeof picked === 'string') {
			isBusy = true;
			try {
				app = await invoke<AppSnapshot>('set_workspace', { workspacePath: picked });
				message = 'Workspace connected.';
			} finally {
				isBusy = false;
			}
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
				const title =
					extension === 'md'
						? 'New note'
						: extension === 'tex'
							? 'New LaTeX Document'
							: extension === 'ipynb'
								? 'New Jupyter Notebook'
								: extension === 'epub'
									? 'New EPUB Book'
									: 'New note';
				const note = await invoke<NoteDocument>('create_note', { title, extension, notebook });
				upsertNoteIntoLibrary(note);
			}
			await refreshApp();
		} finally {
			isBusy = false;
			createLoopRunning = false;
		}
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
		} finally {
			isBusy = false;
		}
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

	// First click selects a row and shows its details in the dashboard; clicking
	// the selected row again opens the note.
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
		} catch (e) {
			console.error(e);
		} finally {
			isBusy = false;
			deleteDialog?.close();
			noteToDelete = null;
		}
	}

	function workspaceLabel(path: string) {
		const parts = path.replace(/\\/g, '/').split('/');
		return parts[parts.length - 1] || path;
	}

	function modelFileName(path: string | null | undefined) {
		if (!path) return 'none';
		const normalized = path.replace(/\\/g, '/');
		return normalized.split('/').pop() || path;
	}

	async function reloadTasks() {
		if (!currentWorkspaceForTasks) return;
		try {
			dashTasks = await taskController.load(currentWorkspaceForTasks);
			tasksLoadedWorkspace = currentWorkspaceForTasks;
		} catch (error) {
			message = String(error);
		}
	}

	createHomeLifecycle({
		get app() { return app; },
		set app(value) { app = value; },
		get appVersion() { return appVersion; },
		set appVersion(value) { appVersion = value; },
		get provider() { return provider; },
		set provider(value) { provider = value; },
		get indexing() { return indexing; },
		set indexing(value) { indexing = value; },
		get ready() { return ready; },
		set ready(value) { ready = value; },
		get message() { return message; },
		set message(value) { message = value; },
		get currentWorkspaceForTasks() { return currentWorkspaceForTasks; },
		get dashTasks() { return dashTasks; },
		set dashTasks(value) { dashTasks = value; },
		reloadTasks,
		disposeTasks: () => taskController.dispose(),
		refreshApp
	});

	let globalSearchDialog: HTMLDialogElement | undefined = $state();
	let globalSearchQuery = $state('');
	let globalSelectedIndex = $state(0);

	let filteredGlobalNotes = $derived.by(() => {
		if (!app?.notes) return [];
		const q = globalSearchQuery.trim().toLowerCase();
		if (!q) return app.notes;
		return app.notes.filter(
			(n) =>
				n.title.toLowerCase().includes(q) ||
				n.relativePath.toLowerCase().includes(q) ||
				n.tags.some((t) => t.toLowerCase().includes(q))
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

	function autoResize(node: HTMLTextAreaElement) {
		const resize = () => {
			node.style.height = 'auto';
			node.style.height = node.scrollHeight + 'px';
		};
		node.addEventListener('input', resize);
		setTimeout(resize, 0);
		return {
			destroy() {
				node.removeEventListener('input', resize);
			}
		};
	}

	return {
		get appVersion() { return appVersion; }, set appVersion(value) { appVersion = value; },
		get app() { return app; }, set app(value) { app = value; },
		get ready() { return ready; }, set ready(value) { ready = value; },
		get indexing() { return indexing; }, set indexing(value) { indexing = value; },
		get provider() { return provider; }, set provider(value) { provider = value; },
		get embeddingModelPath() { return embeddingModelPath; }, set embeddingModelPath(value) { embeddingModelPath = value; },
		get query() { return query; }, set query(value) { query = value; },
		get isBusy() { return isBusy; }, set isBusy(value) { isBusy = value; },
		get message() { return message; }, set message(value) { message = value; },
		get searchResults() { return searchResults; }, set searchResults(value) { searchResults = value; },
		get activeMenuId() { return activeMenuId; }, set activeMenuId(value) { activeMenuId = value; },
		get showAddMenu() { return showAddMenu; }, set showAddMenu(value) { showAddMenu = value; },
		get deleteDialog() { return deleteDialog; }, set deleteDialog(value) { deleteDialog = value; },
		get noteToDelete() { return noteToDelete; }, set noteToDelete(value) { noteToDelete = value; },
		get notebookDialog() { return notebookDialog; }, set notebookDialog(value) { notebookDialog = value; },
		get newNotebookName() { return newNotebookName; }, set newNotebookName(value) { newNotebookName = value; },
		get dashTasks() { return dashTasks; }, set dashTasks(value) { dashTasks = value; },
		get expandedTaskId() { return expandedTaskId; }, set expandedTaskId(value) { expandedTaskId = value; },
		get currentWorkspaceForTasks() { return currentWorkspaceForTasks; }, set currentWorkspaceForTasks(value) { currentWorkspaceForTasks = value; },
		get pinnedNoteIds() { return pinnedNoteIds; }, set pinnedNoteIds(value) { pinnedNoteIds = value; },
		get showTimeline() { return showTimeline; }, set showTimeline(value) { showTimeline = value; },
		get tasksCollapsed() { return tasksCollapsed; }, set tasksCollapsed(value) { tasksCollapsed = value; },
		get storageIssues() { return [...(app?.storageIssues ?? []), ...browserStorageIssues]; },
		get newTaskText() { return newTaskText; }, set newTaskText(value) { newTaskText = value; },
		get isClusterDialogOpen() { return isClusterDialogOpen; }, set isClusterDialogOpen(value) { isClusterDialogOpen = value; },
		get selectedCluster() { return selectedCluster; }, set selectedCluster(value) { selectedCluster = value; },
		get activeTypeFilter() { return activeTypeFilter; }, set activeTypeFilter(value) { activeTypeFilter = value; },
		get notesExpanded() { return notesExpanded; }, set notesExpanded(value) { notesExpanded = value; },
		get docsExpanded() { return docsExpanded; }, set docsExpanded(value) { docsExpanded = value; },
		get notebooksExpanded() { return notebooksExpanded; }, set notebooksExpanded(value) { notebooksExpanded = value; },
		get showNotebookMenu() { return showNotebookMenu; }, set showNotebookMenu(value) { showNotebookMenu = value; },
		get isClustersListOpen() { return isClustersListOpen; }, set isClustersListOpen(value) { isClustersListOpen = value; },
		get activeTag() { return activeTag; }, set activeTag(value) { activeTag = value; },
		get selectedNote() { return selectedNote; }, set selectedNote(value) { selectedNote = value; },
		get notebooks() { return notebooks; }, set notebooks(value) { notebooks = value; },
		get activeNotebook() { return activeNotebook; }, set activeNotebook(value) { activeNotebook = value; },
		get activeTaskFilter() { return activeTaskFilter; }, set activeTaskFilter(value) { activeTaskFilter = value; },
		get globalSearchDialog() { return globalSearchDialog; }, set globalSearchDialog(value) { globalSearchDialog = value; },
		get globalSearchQuery() { return globalSearchQuery; }, set globalSearchQuery(value) { globalSearchQuery = value; },
		get globalSelectedIndex() { return globalSelectedIndex; }, set globalSelectedIndex(value) { globalSelectedIndex = value; },
		get visibleNotes() { return visibleNotes; },
		get allNotesSorted() { return allNotesSorted; },
		get dashNotes() { return dashNotes; },
		get NOTE_SUBTYPES() { return NOTE_SUBTYPES; },
		get DOC_SUBTYPES() { return DOC_SUBTYPES; },
		get attachedDocIds() { return attachedDocIds; },
		get typeCounts() { return typeCounts; },
		get filteredNotebook() { return filteredNotebook; },
		get filteredTasks() { return filteredTasks; },
		get tagCounts() { return tagCounts; },
		get commonplaces() { return commonplaces; },
		get uncategorizedCount() { return uncategorizedCount; },
		get createTarget() { return createTarget; },
		get filteredGlobalNotes() { return filteredGlobalNotes; },
		getNoteBadge,
		togglePin,
		openCluster,
		closeClusterDialog,
		addTask,
		removeTask,
		openClustersDialog,
		closeClustersList,
		setTypeFilter,
		toggleTag,
		fullDateTime,
		scrollToSection,
		folderFromRelativePath,
		folderLabel,
		excerptFromBody,
		upsertNoteIntoLibrary,
		pickWorkspace,
		createNote,
		newNotebook,
		confirmNewNotebook,
		loadNotebooks,
		toggleNotebook,
		notebookCount,
		notebookOf,
		importFile,
		runSearch,
		rebuild,
		timeAgo,
		agoLabel,
		openNote,
		selectOrOpen,
		requestDeleteNote,
		confirmDelete,
		workspaceLabel,
		modelFileName,
		handleGlobalSearchKeydown,
		openNoteFromSearch,
		autofocus,
		autoResize
	};
}
export type HomeController = ReturnType<typeof createHomeController>;
