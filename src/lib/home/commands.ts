import { invoke } from '@tauri-apps/api/core';
import { open } from '@tauri-apps/plugin-dialog';
import type { AppSnapshot, NoteDocument, SearchResponse } from '$lib/types';
import type { TaskItem } from '$lib/tasks/types';

type HomeCommandPort = {
	get app(): AppSnapshot | null;
	set app(value: AppSnapshot | null);
	get isBusy(): boolean;
	set isBusy(value: boolean);
	get message(): string;
	set message(value: string);
	get query(): string;
	get searchResults(): SearchResponse | null;
	set searchResults(value: SearchResponse | null);
	get showAddMenu(): boolean;
	set showAddMenu(value: boolean);
	get createTarget(): string | null;
	get newNotebookName(): string;
	set newNotebookName(value: string);
	get notebookDialog(): HTMLDialogElement | undefined;
	set notebooks(value: string[]);
	set activeNotebook(value: string | null);
	get pendingCreateCount(): number;
	set pendingCreateCount(value: number);
	get createLoopRunning(): boolean;
	set createLoopRunning(value: boolean);
	get noteToDelete(): string | null;
	set noteToDelete(value: string | null);
	get deleteDialog(): HTMLDialogElement | undefined;
	get currentWorkspaceForTasks(): string | null;
	get dashTasks(): TaskItem[];
	set dashTasks(value: TaskItem[]);
	set tasksLoadedWorkspace(value: string | null);
	refreshApp(): Promise<void>;
	loadTasks(workspace: string): Promise<TaskItem[]>;
	upsertNoteIntoLibrary(note: NoteDocument): void;
};

export function createHomeCommands(port: HomeCommandPort) {
	async function pickWorkspace() {
		const picked = await open({
			directory: true,
			multiple: false,
			title: 'Choose your markdown workspace'
		});
		if (typeof picked !== 'string') return;
		port.isBusy = true;
		try {
			port.app = await invoke<AppSnapshot>('set_workspace', { workspacePath: picked });
			port.message = 'Workspace connected.';
		} finally {
			port.isBusy = false;
		}
	}

	async function importFile() {
		port.showAddMenu = false;
		const picked = await open({
			multiple: false,
			filters: [{ name: 'Documents', extensions: ['pdf', 'epub'] }]
		});
		if (typeof picked !== 'string') return;
		port.isBusy = true;
		try {
			await invoke('import_pdf_file', { filePath: picked, notebook: port.createTarget });
			await port.refreshApp();
		} catch (error) {
			console.error('import failed', error);
		} finally {
			port.isBusy = false;
		}
	}

	async function runSearch() {
		port.searchResults = port.query.trim()
			? await invoke<SearchResponse>('search_notes', { query: port.query })
			: null;
	}

	async function rebuild() {
		port.isBusy = true;
		try {
			port.app = await invoke<AppSnapshot>('rebuild_index');
			port.message = 'Index rebuilt.';
		} finally {
			port.isBusy = false;
		}
	}

	async function confirmDelete() {
		if (!port.noteToDelete) return;
		port.isBusy = true;
		try {
			port.app = await invoke<AppSnapshot>('delete_note', { noteId: port.noteToDelete });
		} catch (error) {
			console.error(error);
		} finally {
			port.isBusy = false;
			port.deleteDialog?.close();
			port.noteToDelete = null;
		}
	}

	async function reloadTasks() {
		const workspace = port.currentWorkspaceForTasks;
		if (!workspace) return;
		try {
			port.dashTasks = await port.loadTasks(workspace);
			port.tasksLoadedWorkspace = workspace;
		} catch (error) {
			port.message = String(error);
		}
	}

	async function createNote(extension = 'md', notebook = port.createTarget) {
		port.pendingCreateCount += 1;
		if (port.createLoopRunning) return;
		port.createLoopRunning = true;
		port.isBusy = true;
		try {
			while (port.pendingCreateCount > 0) {
				port.pendingCreateCount -= 1;
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
				port.upsertNoteIntoLibrary(note);
			}
			await port.refreshApp();
		} finally {
			port.isBusy = false;
			port.createLoopRunning = false;
		}
	}

	function newNotebook() {
		port.newNotebookName = '';
		port.notebookDialog?.showModal();
	}

	async function confirmNewNotebook() {
		const name = port.newNotebookName.trim();
		if (!name) return;
		port.isBusy = true;
		try {
			port.notebooks = await invoke<string[]>('create_notebook', { name });
			port.activeNotebook = name;
			port.notebookDialog?.close();
		} catch (error) {
			console.error('create notebook failed', error);
			port.message = `Could not create notebook: ${error}`;
		} finally {
			port.isBusy = false;
		}
	}

	async function loadNotebooks() {
		try {
			port.notebooks = await invoke<string[]>('list_notebooks');
		} catch (error) {
			console.error('list notebooks failed', error);
		}
	}

	function toggleNotebook(notebook: string) {
		port.activeNotebook = port.activeNotebook === notebook ? null : notebook;
	}

	return {
		pickWorkspace,
		importFile,
		runSearch,
		rebuild,
		confirmDelete,
		reloadTasks,
		createNote,
		newNotebook,
		confirmNewNotebook,
		loadNotebooks,
		toggleNotebook
	};
}
