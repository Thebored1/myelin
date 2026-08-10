import type { Task, TaskItem } from './types';

function now() {
	return new Date().toISOString();
}

function idOf(value: string | number | undefined, fallback: string) {
	if (typeof value === 'number') return `legacy-${value}`;
	return value?.trim() || fallback;
}

function dueFromLegacy(task: TaskItem) {
	const date = task.dueDate?.trim();
	const time = task.dueTime?.trim();
	if (date && time) return `${date}T${time}`;
	return date || time || null;
}

export function toTaskItem(task: Task): TaskItem {
	const due = task.due ?? '';
	const [dueDate, dueTime] = due.includes('T') ? due.split('T', 2) : [due, ''];
	return {
		id: task.id,
		text: task.title,
		details: task.details,
		done: task.done,
		dueDate,
		dueTime,
		notebook: task.notebook ?? undefined,
		subtasks: task.subtasks.map((subtask) => ({
			id: subtask.id,
			text: subtask.text,
			done: subtask.done
		}))
	};
}

export function toTask(task: TaskItem, existing?: Task): Task {
	const fallback = `legacy-${Date.now()}`;
	const taskId = idOf(task.id, fallback);
	const legacyTaskId = typeof task.id === 'number' ? String(task.id) : taskId;
	return {
		id: taskId,
		title: task.text.trim(),
		details: task.details?.trim() ?? '',
		notebook: task.notebook?.trim() || null,
		due: dueFromLegacy(task),
		done: Boolean(task.done),
		subtasks: (task.subtasks ?? []).map((subtask, index) => ({
			id:
			typeof subtask.id === 'number'
					? `legacy-${legacyTaskId}-${subtask.id}`
					: idOf(subtask.id, `${taskId}-subtask-${index}`),
			text: subtask.text,
			done: Boolean(subtask.done)
		})),
		position: existing?.position ?? null,
		createdAt: existing?.createdAt || now(),
		updatedAt: existing?.updatedAt || now()
	};
}

export function normalizeLegacyTask(value: unknown, index: number): Task | null {
	if (!value || typeof value !== 'object') return null;
	const legacy = value as Partial<TaskItem>;
	if (typeof legacy.text !== 'string' || !legacy.text.trim()) return null;
	return toTask(
		{
			id: legacy.id ?? index,
			text: legacy.text,
			details: legacy.details,
			done: Boolean(legacy.done),
			dueDate: legacy.dueDate,
			dueTime: legacy.dueTime,
			notebook: legacy.notebook,
			subtasks: legacy.subtasks
		},
		undefined
	);
}

export function normalizedTaskContent(task: Task) {
	return JSON.stringify({
		title: task.title,
		details: task.details,
		notebook: task.notebook,
		due: task.due,
		done: task.done,
		subtasks: task.subtasks
	});
}
