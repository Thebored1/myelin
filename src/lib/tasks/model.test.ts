import { describe, expect, it } from 'vitest';
import { normalizeLegacyTask, normalizedTaskContent, toTaskItem } from './model';

describe('task persistence mapping', () => {
	it('converts legacy task fields without losing deadlines or subtasks', () => {
		const task = normalizeLegacyTask(
			{
				id: 12,
				text: 'Review paper',
				details: 'Check figures',
				dueDate: '2026-08-11',
				dueTime: '09:30',
				notebook: 'research',
				subtasks: [{ id: 3, text: 'Figure 1', done: true }]
			},
			0
		);

		expect(task).toMatchObject({
			id: 'legacy-12',
			title: 'Review paper',
			due: '2026-08-11T09:30',
			notebook: 'research'
		});
		expect(task?.subtasks[0]).toEqual({ id: 'legacy-12-3', text: 'Figure 1', done: true });
		expect(toTaskItem(task!)).toMatchObject({
			text: 'Review paper',
			dueDate: '2026-08-11',
			dueTime: '09:30'
		});
	});

	it('uses normalized content for idempotent duplicate detection', () => {
		const left = normalizeLegacyTask({ id: 1, text: 'Same' }, 0)!;
		const right = { ...left, id: 'different-id' };
		expect(normalizedTaskContent(left)).toBe(normalizedTaskContent(right));
	});
});
