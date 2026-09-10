import type { NoteDocument, NoteSummary } from '$lib/types';

export function getNoteBadge(note: NoteSummary) {
	const rel = note.relativePath.toLowerCase();
	if (rel.endsWith('.pdf')) return 'pdf';
	if (rel.endsWith('.epub')) return 'epub';
	if (rel.endsWith('.tex')) return 'tex';
	if (rel.endsWith('.ipynb')) return 'jupyter';
	if (note.sourcePdf) return 'note + pdf';
	return 'note';
}

export function fullDateTime(value: string) {
	const d = new Date(value);
	return (
		d.toLocaleDateString('en-US', { month: 'long', day: 'numeric', year: 'numeric' }) +
		' ' +
		d.toLocaleTimeString('en-US', { hour: 'numeric', minute: '2-digit' })
	);
}

export function folderFromRelativePath(relativePath: string) {
	const segments = relativePath.split('/').filter(Boolean);
	return segments.length > 1 ? segments.slice(0, -1).join('/') : 'Root';
}

export function excerptFromBody(body: string) {
	const flat = body.trim().replace(/\s+/g, ' ');
	return flat.length > 400 ? `${flat.slice(0, 400)}...` : flat;
}

export function notebookCount(notes: NoteSummary[], notebook: string) {
	return notes.filter((note) => note.folder === notebook || note.folder.startsWith(notebook + '/'))
		.length;
}

export function notebookOf(note: NoteSummary): string | null {
	if (!note.folder || note.folder === 'Root') return null;
	return note.folder.split('/')[0];
}

export function timeAgo(value: string) {
	const diff = Date.now() - new Date(value).getTime();
	const mins = Math.floor(diff / 60000);
	if (mins < 1) return 'now';
	if (mins < 60) return `${mins}m`;
	const hrs = Math.floor(mins / 60);
	if (hrs < 24) return `${hrs}h`;
	const days = Math.floor(hrs / 24);
	if (days < 7) return `${days}d`;
	return `${Math.floor(days / 7)}w`;
}

export function agoLabel(value: string) {
	const text = timeAgo(value);
	return text === 'now' ? 'just now' : `${text} ago`;
}

export function workspaceLabel(path: string) {
	const parts = path.replace(/\\/g, '/').split('/');
	return parts[parts.length - 1] || path;
}

export function modelFileName(path: string | null | undefined) {
	if (!path) return 'none';
	const normalized = path.replace(/\\/g, '/');
	return normalized.split('/').pop() || path;
}

export function folderLabel(note: NoteSummary, workspacePath: string | null | undefined) {
	const segments = note.relativePath.replace(/\\/g, '/').split('/').filter(Boolean);
	if (segments.length > 1) return segments.slice(0, -1).join('/');
	return workspacePath ? workspaceLabel(workspacePath) : 'workspace';
}

export function noteSummaryFromDocument(note: NoteDocument): NoteSummary {
	return {
		id: note.id,
		title: note.title,
		tags: note.tags,
		folder: folderFromRelativePath(note.relativePath),
		excerpt: excerptFromBody(note.body),
		relativePath: note.relativePath,
		createdAt: note.createdAt,
		updatedAt: note.updatedAt,
		backlinks: note.backlinks
	};
}
