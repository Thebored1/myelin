export type TaskSubtask = {
	id: string | number;
	text: string;
	done: boolean;
};

export type TaskItem = {
	id: string | number;
	text: string;
	details?: string;
	done: boolean;
	dueDate?: string;
	dueTime?: string;
	notebook?: string;
	subtasks?: TaskSubtask[];
};

export type Task = {
	id: string;
	title: string;
	details: string;
	notebook: string | null;
	due: string | null;
	done: boolean;
	subtasks: { id: string; text: string; done: boolean }[];
	position: number | null;
	createdAt: string;
	updatedAt: string;
};
