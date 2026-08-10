import type { NoteSummary } from '$lib/types';
import type { NoteFilter, NoteType } from './types';

export const NOTE_GROUP: readonly NoteType[] = ['md', 'tex', 'ipynb'];
export const DOCUMENT_GROUP: readonly NoteType[] = ['pdf', 'epub'];

export function noteType(note: Pick<NoteSummary, 'relativePath'>): NoteType {
	const path = note.relativePath.toLowerCase();
	if (path.endsWith('.pdf')) return 'pdf';
	if (path.endsWith('.epub')) return 'epub';
	if (path.endsWith('.tex')) return 'tex';
	if (path.endsWith('.ipynb')) return 'ipynb';
	return 'md';
}

export function isAttachmentCopy(note: Pick<NoteSummary, 'relativePath' | 'id'>, referencedIds: ReadonlySet<string>): boolean {
	if (!referencedIds.has(note.id)) return false;
	const name = note.relativePath.split(/[\\/]/).pop()?.toLowerCase() ?? '';
	return / \d+\.(pdf|epub)$/.test(name) || / [0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}\.(pdf|epub)$/.test(name);
}

export function notebookOf(note: Pick<NoteSummary, 'folder'>): string | null {
	return note.folder.trim() ? note.folder : null;
}

export function filterNotes(
	notes: readonly NoteSummary[],
	filter: NoteFilter,
	tag: string | null,
	notebook: string | null,
	attachmentIds: ReadonlySet<string>,
	pinnedIds: readonly string[]
): NoteSummary[] {
	let result = notes.filter((note) => {
		const type = noteType(note);
		if (filter === 'notes' && !NOTE_GROUP.includes(type)) return false;
		if (filter === 'documents' && (!DOCUMENT_GROUP.includes(type) || attachmentIds.has(note.id))) return false;
		if (filter !== 'all' && filter !== 'notes' && filter !== 'documents' && type !== filter) return false;
		if (tag !== null && !note.tags.includes(tag)) return false;
		if (notebook === null) return notebookOf(note) === null;
		return note.folder === notebook || note.folder.startsWith(`${notebook}/`);
	});
	const pinned = new Set(pinnedIds);
	return result.sort((a, b) => Number(pinned.has(b.id)) - Number(pinned.has(a.id)));
}

export function timeAgo(value: string, now = Date.now()): string {
	const mins = Math.floor((now - new Date(value).getTime()) / 60000);
	if (mins < 1) return 'now';
	if (mins < 60) return `${mins}m`;
	const hours = Math.floor(mins / 60);
	if (hours < 24) return `${hours}h`;
	const days = Math.floor(hours / 24);
	if (days < 7) return `${days}d`;
	return `${Math.floor(days / 7)}w`;
}
