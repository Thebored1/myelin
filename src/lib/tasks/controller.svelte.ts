import type { UnlistenFn } from '@tauri-apps/api/event';
import { normalizeLegacyTask, normalizedTaskContent, toTask, toTaskItem } from './model';
import { tauriTaskPort, type TaskPort } from './port';
import type { Task, TaskItem } from './types';

const migrationKey = (workspace: string) => `tasks_native_migrated_${workspace}`;
const recoveryKey = (workspace: string) => `tasks_${workspace}`;

export class TaskController {
	private readonly port: TaskPort;
	private saveTimer: ReturnType<typeof setTimeout> | undefined;
	private unlistenChanged: UnlistenFn | undefined;
	private disposed = false;
	issue = $state('');

	constructor(port: TaskPort = tauriTaskPort) {
		this.port = port;
	}

	async load(workspace: string): Promise<TaskItem[]> {
		this.issue = '';
		try {
			let native = await this.port.list();
			if (!localStorage.getItem(migrationKey(workspace))) {
				const migrated = this.readLegacyBackup(workspace);
				// These sets are local deduplication helpers, not reactive state.
				// eslint-disable-next-line svelte/prefer-svelte-reactivity
				const existingIds = new Set(native.map((task) => task.id));
				// eslint-disable-next-line svelte/prefer-svelte-reactivity
				const existingContent = new Set(native.map(normalizedTaskContent));
				for (const task of migrated) {
					if (existingIds.has(task.id) || existingContent.has(normalizedTaskContent(task)))
						continue;
					const saved = await this.port.save(task);
					native = [...native, saved];
					existingIds.add(saved.id);
					existingContent.add(normalizedTaskContent(saved));
				}
				localStorage.setItem(migrationKey(workspace), 'true');
				native = await this.port.list();
			}
			return native.map(toTaskItem);
		} catch (error) {
			this.issue = `Tasks could not be loaded: ${String(error)}`;
			throw error;
		}
	}

	private readLegacyBackup(workspace: string): Task[] {
		const raw = localStorage.getItem(recoveryKey(workspace));
		if (!raw) return [];
		try {
			const parsed: unknown = JSON.parse(raw);
			return Array.isArray(parsed)
				? parsed
						.map((value, index) => normalizeLegacyTask(value, index))
						.filter((task): task is Task => task !== null)
				: [];
		} catch (error) {
			this.issue = `Task recovery backup could not be read: ${String(error)}`;
			return [];
		}
	}

	scheduleSave(workspace: string, items: TaskItem[]) {
		if (this.disposed) return;
		if (this.saveTimer) clearTimeout(this.saveTimer);
		this.saveTimer = setTimeout(() => {
			void this.save(workspace, items);
		}, 300);
	}

	async save(workspace: string, items: TaskItem[]): Promise<boolean> {
		try {
			const persisted = await this.port.list();
			const desired = items.map((item) =>
				toTask(
					item,
					persisted.find((task) => task.id === String(item.id))
				)
			);
			for (const task of desired) await this.port.save(task);
			await this.port.emitSync(workspace, 'main');
			this.issue = '';
			return true;
		} catch (error) {
			this.issue = `Tasks were not saved: ${String(error)}`;
			return false;
		}
	}

	async remove(workspace: string, id: string | number): Promise<boolean> {
		this.cancelPendingSave();
		try {
			await this.port.remove(String(id));
			this.cancelPendingSave();
			await this.port.emitSync(workspace, 'main');
			this.issue = '';
			return true;
		} catch (error) {
			this.issue = `Task was not deleted: ${String(error)}`;
			return false;
		}
	}

	cancelPendingSave() {
		if (this.saveTimer) {
			clearTimeout(this.saveTimer);
			this.saveTimer = undefined;
		}
	}

	async attachChanged(callback: () => void) {
		this.unlistenChanged = await this.port.listenChanged(callback);
	}

	dispose() {
		this.disposed = true;
		this.cancelPendingSave();
		this.unlistenChanged?.();
	}
}

export function createTaskController(port?: TaskPort) {
	return new TaskController(port);
}
