import { invoke } from '@tauri-apps/api/core';
import { appCache } from '$lib/appCache';
import type {
	AppSnapshot,
	NoteDocument,
	NoteSummary,
	ProviderStatus,
	SearchResponse
} from '$lib/types';
import { createHomeLifecycle } from './lifecycle.svelte';
import { createHomeDashboard } from './dashboard.svelte';
import { createHomeCommands } from './commands';
import { createHomeNavigation } from './navigation';
import { createBoundController } from '$lib/controllerView';
import {
	attachedDocumentIds,
	commonplaces as findCommonplaces,
	filterNotes,
	tagCounts as countTags,
	typeCounts as countTypes,
	type HomeTypeFilter
} from './filters';
import {
	agoLabel,
	excerptFromBody,
	folderFromRelativePath,
	folderLabel as formatFolderLabel,
	fullDateTime,
	getNoteBadge,
	modelFileName,
	noteSummaryFromDocument,
	notebookCount as countNotesInNotebook,
	notebookOf,
	timeAgo,
	workspaceLabel
} from './presentation';

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

	let expandedTaskId = $state<string | number | null>(null);
	const dashboard = createHomeDashboard({
		get workspacePath() {
			return app?.workspacePath ?? null;
		},
		onWorkspaceChange: () => {
			activeTag = null;
			selectedNote = null;
			activeNotebook = null;
		},
		setMessage: (value) => {
			message = value;
		}
	});
	let isClusterDialogOpen = $state(false);
	let selectedCluster = $state<NoteSummary[]>([]);

	function togglePin(id: string) {
		dashboard.togglePin(id);
		activeMenuId = null;
	}

	function openCluster(cluster: NoteSummary[]) {
		selectedCluster = cluster;
		isClusterDialogOpen = true;
	}
	function closeClusterDialog() {
		isClusterDialogOpen = false;
	}

	const visibleNotes = $derived.by(() => {
		const base = (
			query && searchResults ? searchResults.results.map((r) => r.note) : (app?.notes ?? [])
		) as NoteSummary[];
		return [...base].sort((a, b) => b.createdAt.localeCompare(a.createdAt));
	});

	// Rail details — always based on the full library, unaffected by the rail search
	const allNotesSorted = $derived.by(() =>
		[...(app?.notes ?? [])].sort((a, b) => b.createdAt.localeCompare(a.createdAt))
	);
	const dashNotes = $derived(allNotesSorted);

	// ── Notes list (left) — filter ──
	// Main-pane tabs filter by category ("notes" = editable md/tex/ipynb, "documents"
	// = source pdf/epub); the sidebar dropdowns set a specific type. Both share this.
	type NbFilter = HomeTypeFilter;
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
	const attachedDocIds = $derived.by(() => attachedDocumentIds(app?.notes ?? []));
	const typeCounts = $derived.by(() => countTypes(dashNotes, attachedDocIds));
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
	const filteredNotebook = $derived.by(() =>
		filterNotes(
			dashNotes,
			activeTypeFilter,
			activeTag,
			activeNotebook,
			dashboard.pinnedNoteIds,
			attachedDocIds
		)
	);

	function setTypeFilter(t: typeof activeTypeFilter) {
		activeTypeFilter = t;
	}
	function toggleTag(tag: string) {
		activeTag = activeTag === tag ? null : tag;
	}

	// ── Tasks (right) — filter tabs ──
	let activeTaskFilter = $state<'all' | 'active' | 'done'>('all');
	const filteredTasks = $derived.by(() => {
		if (activeTaskFilter === 'active') return dashboard.dashTasks.filter((t) => !t.done);
		if (activeTaskFilter === 'done') return dashboard.dashTasks.filter((t) => t.done);
		return dashboard.dashTasks;
	});

	const tagCounts = $derived.by(() => countTags(app?.notes ?? []));

	function scrollToSection(id: string) {
		document.getElementById(id)?.scrollIntoView({ behavior: 'smooth', block: 'start' });
	}

	const commonplaces = $derived.by(() => (app?.notes ? findCommonplaces(app.notes) : []));

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
		void homeCommands.loadNotebooks();
	}

	// Keep the cross-navigation cache in sync with any path that reassigns the
	// snapshot (create / delete / rebuild / workspace change), so a later return
	// to Home paints current data, not a stale copy.
	$effect(() => {
		if (app) appCache.app = app;
		if (provider) appCache.provider = provider;
	});

	const upsertNoteIntoLibrary = (note: NoteDocument) => {
		if (!app) return;
		const summary = noteSummaryFromDocument(note);
		const existingIndex = app.notes.findIndex((entry) => entry.id === note.id);
		if (existingIndex >= 0) app.notes[existingIndex] = summary;
		else app.notes = [summary, ...app.notes];
		if (!app.customNoteOrder.includes(note.id))
			app.customNoteOrder = [...app.customNoteOrder, note.id];
		if (!app.libraryFacets.folders.includes(summary.folder))
			app.libraryFacets.folders = [...app.libraryFacets.folders, summary.folder].sort();
		// This is an ephemeral deduplication set, not reactive state.
		// eslint-disable-next-line svelte/prefer-svelte-reactivity
		app.libraryFacets.tags = [...new Set([...app.libraryFacets.tags, ...summary.tags])].sort();
	};
	// The top-level notebook a note belongs to, or null if it's loose in the workspace.
	const uncategorizedCount = $derived(dashNotes.filter((n) => notebookOf(n) === null).length);
	const notebookCount = (notebook: string) => countNotesInNotebook(dashNotes, notebook);

	// Where new notes / uploads land — inferred, never asked: the open note's
	// notebook if one is selected, otherwise the notebook you're viewing. null =
	// uncategorized (workspace root).
	const createTarget = $derived(selectedNote ? notebookOf(selectedNote) : activeNotebook);

	const homeCommands = createHomeCommands({
		get app() {
			return app;
		},
		set app(value) {
			app = value;
		},
		get isBusy() {
			return isBusy;
		},
		set isBusy(value) {
			isBusy = value;
		},
		get message() {
			return message;
		},
		set message(value) {
			message = value;
		},
		get query() {
			return query;
		},
		get searchResults() {
			return searchResults;
		},
		set searchResults(value) {
			searchResults = value;
		},
		get showAddMenu() {
			return showAddMenu;
		},
		set showAddMenu(value) {
			showAddMenu = value;
		},
		get createTarget() {
			return createTarget;
		},
		get newNotebookName() {
			return newNotebookName;
		},
		set newNotebookName(value) {
			newNotebookName = value;
		},
		get notebookDialog() {
			return notebookDialog;
		},
		set notebooks(value: string[]) {
			notebooks = value;
		},
		set activeNotebook(value: string | null) {
			activeNotebook = value;
		},
		get pendingCreateCount() {
			return pendingCreateCount;
		},
		set pendingCreateCount(value) {
			pendingCreateCount = value;
		},
		get createLoopRunning() {
			return createLoopRunning;
		},
		set createLoopRunning(value) {
			createLoopRunning = value;
		},
		get noteToDelete() {
			return noteToDelete;
		},
		set noteToDelete(value) {
			noteToDelete = value;
		},
		get deleteDialog() {
			return deleteDialog;
		},
		get currentWorkspaceForTasks() {
			return dashboard.currentWorkspaceForTasks;
		},
		get dashTasks() {
			return dashboard.dashTasks;
		},
		set dashTasks(value) {
			dashboard.dashTasks = value;
		},
		set tasksLoadedWorkspace(value: string | null) {
			dashboard.tasksLoadedWorkspace = value;
		},
		refreshApp,
		loadTasks: (workspace) => dashboard.loadTasks(workspace),
		upsertNoteIntoLibrary
	});

	createHomeLifecycle({
		get app() {
			return app;
		},
		set app(value) {
			app = value;
		},
		get appVersion() {
			return appVersion;
		},
		set appVersion(value) {
			appVersion = value;
		},
		get provider() {
			return provider;
		},
		set provider(value) {
			provider = value;
		},
		get indexing() {
			return indexing;
		},
		set indexing(value) {
			indexing = value;
		},
		get ready() {
			return ready;
		},
		set ready(value) {
			ready = value;
		},
		get message() {
			return message;
		},
		set message(value) {
			message = value;
		},
		get currentWorkspaceForTasks() {
			return dashboard.currentWorkspaceForTasks;
		},
		get dashTasks() {
			return dashboard.dashTasks;
		},
		set dashTasks(value) {
			dashboard.dashTasks = value;
		},
		reloadTasks: homeCommands.reloadTasks,
		disposeTasks: () => dashboard.disposeTasks(),
		refreshApp
	});

	let globalSearchDialog: HTMLDialogElement | undefined = $state();
	let globalSearchQuery = $state('');
	let globalSelectedIndex = $state(0);

	const filteredGlobalNotes = $derived.by(() => {
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

	const homeNavigation = createHomeNavigation({
		get selectedNote() {
			return selectedNote;
		},
		set selectedNote(value) {
			selectedNote = value;
		},
		set tasksCollapsed(value: boolean) {
			dashboard.tasksCollapsed = value;
		},
		set activeMenuId(value: string | null) {
			activeMenuId = value;
		},
		set noteToDelete(value: string | null) {
			noteToDelete = value;
		},
		get deleteDialog() {
			return deleteDialog;
		},
		get globalSearchDialog() {
			return globalSearchDialog;
		},
		get filteredGlobalNotes() {
			return filteredGlobalNotes;
		},
		get globalSelectedIndex() {
			return globalSelectedIndex;
		},
		set globalSelectedIndex(value) {
			globalSelectedIndex = value;
		}
	});

	return createBoundController(
		() => ({
			appVersion: (() => {
				return appVersion;
			})(),
			app: (() => {
				return app;
			})(),
			ready: (() => {
				return ready;
			})(),
			indexing: (() => {
				return indexing;
			})(),
			provider: (() => {
				return provider;
			})(),
			embeddingModelPath: (() => {
				return embeddingModelPath;
			})(),
			query: (() => {
				return query;
			})(),
			isBusy: (() => {
				return isBusy;
			})(),
			message: (() => {
				return message;
			})(),
			searchResults: (() => {
				return searchResults;
			})(),
			activeMenuId: (() => {
				return activeMenuId;
			})(),
			showAddMenu: (() => {
				return showAddMenu;
			})(),
			deleteDialog: (() => {
				return deleteDialog;
			})(),
			noteToDelete: (() => {
				return noteToDelete;
			})(),
			notebookDialog: (() => {
				return notebookDialog;
			})(),
			newNotebookName: (() => {
				return newNotebookName;
			})(),
			dashTasks: (() => {
				return dashboard.dashTasks;
			})(),
			expandedTaskId: (() => {
				return expandedTaskId;
			})(),
			currentWorkspaceForTasks: (() => {
				return dashboard.currentWorkspaceForTasks;
			})(),
			pinnedNoteIds: (() => {
				return dashboard.pinnedNoteIds;
			})(),
			showTimeline: (() => {
				return dashboard.showTimeline;
			})(),
			tasksCollapsed: (() => {
				return dashboard.tasksCollapsed;
			})(),
			storageIssues: (() => {
				return [...(app?.storageIssues ?? []), ...dashboard.storageIssues];
			})(),
			newTaskText: (() => {
				return dashboard.newTaskText;
			})(),
			isClusterDialogOpen: (() => {
				return isClusterDialogOpen;
			})(),
			selectedCluster: (() => {
				return selectedCluster;
			})(),
			activeTypeFilter: (() => {
				return activeTypeFilter;
			})(),
			notesExpanded: (() => {
				return notesExpanded;
			})(),
			docsExpanded: (() => {
				return docsExpanded;
			})(),
			notebooksExpanded: (() => {
				return notebooksExpanded;
			})(),
			showNotebookMenu: (() => {
				return showNotebookMenu;
			})(),
			isClustersListOpen: (() => {
				return isClustersListOpen;
			})(),
			activeTag: (() => {
				return activeTag;
			})(),
			selectedNote: (() => {
				return selectedNote;
			})(),
			notebooks: (() => {
				return notebooks;
			})(),
			activeNotebook: (() => {
				return activeNotebook;
			})(),
			activeTaskFilter: (() => {
				return activeTaskFilter;
			})(),
			globalSearchDialog: (() => {
				return globalSearchDialog;
			})(),
			globalSearchQuery: (() => {
				return globalSearchQuery;
			})(),
			globalSelectedIndex: (() => {
				return globalSelectedIndex;
			})(),
			visibleNotes: (() => {
				return visibleNotes;
			})(),
			allNotesSorted: (() => {
				return allNotesSorted;
			})(),
			dashNotes: (() => {
				return dashNotes;
			})(),
			NOTE_SUBTYPES: (() => {
				return NOTE_SUBTYPES;
			})(),
			DOC_SUBTYPES: (() => {
				return DOC_SUBTYPES;
			})(),
			attachedDocIds: (() => {
				return attachedDocIds;
			})(),
			typeCounts: (() => {
				return typeCounts;
			})(),
			filteredNotebook: (() => {
				return filteredNotebook;
			})(),
			filteredTasks: (() => {
				return filteredTasks;
			})(),
			tagCounts: (() => {
				return tagCounts;
			})(),
			commonplaces: (() => {
				return commonplaces;
			})(),
			uncategorizedCount: (() => {
				return uncategorizedCount;
			})(),
			createTarget: (() => {
				return createTarget;
			})(),
			filteredGlobalNotes: (() => {
				return filteredGlobalNotes;
			})()
		}),
		{
			appVersion: (value) => {
				appVersion = value;
			},
			app: (value) => {
				app = value;
			},
			ready: (value) => {
				ready = value;
			},
			indexing: (value) => {
				indexing = value;
			},
			provider: (value) => {
				provider = value;
			},
			embeddingModelPath: (value) => {
				embeddingModelPath = value;
			},
			query: (value) => {
				query = value;
			},
			isBusy: (value) => {
				isBusy = value;
			},
			message: (value) => {
				message = value;
			},
			searchResults: (value) => {
				searchResults = value;
			},
			activeMenuId: (value) => {
				activeMenuId = value;
			},
			showAddMenu: (value) => {
				showAddMenu = value;
			},
			deleteDialog: (value) => {
				deleteDialog = value;
			},
			noteToDelete: (value) => {
				noteToDelete = value;
			},
			notebookDialog: (value) => {
				notebookDialog = value;
			},
			newNotebookName: (value) => {
				newNotebookName = value;
			},
			dashTasks: (value) => {
				dashboard.dashTasks = value;
			},
			expandedTaskId: (value) => {
				expandedTaskId = value;
			},
			currentWorkspaceForTasks: (value) => {
				dashboard.currentWorkspaceForTasks = value;
			},
			pinnedNoteIds: (value) => {
				dashboard.pinnedNoteIds = value;
			},
			showTimeline: (value) => {
				dashboard.showTimeline = value;
			},
			tasksCollapsed: (value) => {
				dashboard.tasksCollapsed = value;
			},
			newTaskText: (value) => {
				dashboard.newTaskText = value;
			},
			isClusterDialogOpen: (value) => {
				isClusterDialogOpen = value;
			},
			selectedCluster: (value) => {
				selectedCluster = value;
			},
			activeTypeFilter: (value) => {
				activeTypeFilter = value;
			},
			notesExpanded: (value) => {
				notesExpanded = value;
			},
			docsExpanded: (value) => {
				docsExpanded = value;
			},
			notebooksExpanded: (value) => {
				notebooksExpanded = value;
			},
			showNotebookMenu: (value) => {
				showNotebookMenu = value;
			},
			isClustersListOpen: (value) => {
				isClustersListOpen = value;
			},
			activeTag: (value) => {
				activeTag = value;
			},
			selectedNote: (value) => {
				selectedNote = value;
			},
			notebooks: (value) => {
				notebooks = value;
			},
			activeNotebook: (value) => {
				activeNotebook = value;
			},
			activeTaskFilter: (value) => {
				activeTaskFilter = value;
			},
			globalSearchDialog: (value) => {
				globalSearchDialog = value;
			},
			globalSearchQuery: (value) => {
				globalSearchQuery = value;
			},
			globalSelectedIndex: (value) => {
				globalSelectedIndex = value;
			}
		},
		{
			getNoteBadge,
			togglePin,
			openCluster,
			closeClusterDialog,
			addTask: dashboard.addTask,
			removeTask: dashboard.removeTask,
			openClustersDialog,
			closeClustersList,
			setTypeFilter,
			toggleTag,
			fullDateTime,
			scrollToSection,
			folderFromRelativePath,
			folderLabel: (note: NoteSummary) => formatFolderLabel(note, app?.workspacePath),
			excerptFromBody,
			upsertNoteIntoLibrary,
			...homeCommands,
			notebookCount,
			notebookOf,
			timeAgo,
			agoLabel,
			...homeNavigation,
			workspaceLabel,
			modelFileName
		}
	);
}
export type HomeController = ReturnType<typeof createHomeController>;
