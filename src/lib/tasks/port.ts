import { invoke } from '@tauri-apps/api/core';
import { emit, listen, type UnlistenFn } from '@tauri-apps/api/event';
import type { Task } from './types';

export type TaskPort = {
	list(): Promise<Task[]>;
	save(task: Task): Promise<Task>;
	remove(id: string): Promise<void>;
	listenChanged(callback: () => void): Promise<UnlistenFn>;
	emitSync(workspacePath: string, source: string): Promise<void>;
};

export const tauriTaskPort: TaskPort = {
	list: () => invoke<Task[]>('list_tasks'),
	save: (task) => invoke<Task>('save_task', { task }),
	remove: (id) => invoke<void>('delete_task', { id }),
	listenChanged: (callback) => listen('tasks://changed', callback),
	emitSync: (workspacePath, source) => emit('tasks://sync', { workspacePath, source })
};
