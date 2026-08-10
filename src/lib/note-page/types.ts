import type { NoteSnapshot } from '$lib/types';

export type AiInteractionMode = 'chat' | 'write';

export type AiEditTarget = {
	text: string;
	before: string;
	after: string;
	start?: number;
	end?: number;
	cursor?: boolean;
};

export type ActiveSection = { key: string; label?: string; content: string };

export type SectionCacheState = {
	done: number;
	total: number;
	sectionDone: number;
	sectionTotal: number;
	label: string;
	profile: string;
	startedAt: number;
	finished: boolean;
	failed: number;
	failedDetails: string[];
	elapsedMs: number | null;
};

export type DebugTraceEntry = { time: number; msg: string; kind: string };

export type BlockItem = {
	id: string;
	original: string;
	text: string;
	sourceNoteId: string;
	sourceTitle: string;
};

export type ChatSnapshot = NoteSnapshot;

