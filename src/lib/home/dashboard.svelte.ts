import type { StorageIssue } from '$lib/types';
import { createTaskController } from '$lib/tasks/controller.svelte';
import type { TaskItem } from '$lib/tasks/types';
import { readBrowserStorage } from './storage';

// Dashboard panel state: quick tasks, pinned notes, and the per-workspace
// browser preferences (timeline visibility, tasks collapse). Persistence is
// workspace-scoped — switching workspaces reloads tasks and stored preferences.
export function createHomeDashboard(opts: {
	get workspacePath(): string | null;
	onWorkspaceChange: () => void;
	setMessage: (message: string) => void;
}) {
	let dashTasks = $state<TaskItem[]>([]);
	let currentWorkspaceForTasks = $state<string | null>(null);
	let tasksLoadedWorkspace = $state<string | null>(null);
	let pinnedNoteIds = $state<string[]>([]);
	let showTimeline = $state(true);
	let tasksCollapsed = $state(false);
	let browserStorageIssues = $state<StorageIssue[]>([]);
	let newTaskText = $state('');
	const taskController = createTaskController();

	function reportBrowserStorageIssue(issueOrKey: StorageIssue | string, error: unknown) {
		const issue: StorageIssue =
			typeof issueOrKey === 'string'
				? {
						code: 'browser-storage',
						severity: 'warning',
						path: issueOrKey,
						message:
							'A browser preference could not be saved; the rest of the library remains available.',
						recoverable: true
					}
				: issueOrKey;
		if (!browserStorageIssues.some((existing) => existing.path === issue.path)) {
			browserStorageIssues = [...browserStorageIssues, issue];
		}
		console.warn(`Could not persist browser preference ${issue.path}`, error);
	}

	$effect(() => {
		const workspacePath = opts.workspacePath;
		if (workspacePath && workspacePath !== currentWorkspaceForTasks) {
			currentWorkspaceForTasks = workspacePath;
			browserStorageIssues = [];
			tasksLoadedWorkspace = null;
			opts.onWorkspaceChange();
			void taskController
				.load(workspacePath)
				.then((tasks) => {
					if (currentWorkspaceForTasks === workspacePath) {
						dashTasks = tasks;
						tasksLoadedWorkspace = workspacePath;
					}
				})
				.catch((error) => {
					opts.setMessage(String(error));
					dashTasks = [];
				});
			const pinnedKey = `pinned_${workspacePath}`;
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
			const timelineKey = `timeline_${workspacePath}`;
			const storedTimeline = readBrowserStorage(timelineKey, reportBrowserStorageIssue);
			if (storedTimeline !== null) {
				showTimeline = storedTimeline === 'true';
			} else {
				showTimeline = true;
			}
			tasksCollapsed =
				readBrowserStorage(`taskscollapsed_${workspacePath}`, reportBrowserStorageIssue) === 'true';
		}
	});

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

	function togglePin(id: string) {
		if (pinnedNoteIds.includes(id)) {
			pinnedNoteIds = pinnedNoteIds.filter((pid) => pid !== id);
		} else {
			pinnedNoteIds = [...pinnedNoteIds, id];
		}
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

	return {
		get dashTasks() {
			return dashTasks;
		},
		set dashTasks(value: TaskItem[]) {
			dashTasks = value;
		},
		get currentWorkspaceForTasks() {
			return currentWorkspaceForTasks;
		},
		set currentWorkspaceForTasks(value: string | null) {
			currentWorkspaceForTasks = value;
		},
		set tasksLoadedWorkspace(value: string | null) {
			tasksLoadedWorkspace = value;
		},
		get pinnedNoteIds() {
			return pinnedNoteIds;
		},
		set pinnedNoteIds(value: string[]) {
			pinnedNoteIds = value;
		},
		get showTimeline() {
			return showTimeline;
		},
		set showTimeline(value: boolean) {
			showTimeline = value;
		},
		get tasksCollapsed() {
			return tasksCollapsed;
		},
		set tasksCollapsed(value: boolean) {
			tasksCollapsed = value;
		},
		get newTaskText() {
			return newTaskText;
		},
		set newTaskText(value: string) {
			newTaskText = value;
		},
		get storageIssues() {
			return browserStorageIssues;
		},
		togglePin,
		addTask,
		removeTask,
		loadTasks: (workspace: string) => taskController.load(workspace),
		disposeTasks: () => taskController.dispose()
	};
}
