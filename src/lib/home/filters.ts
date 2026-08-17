import type { NoteSummary } from '$lib/types';
import { DOCUMENT_GROUP as DOC_GROUP, NOTE_GROUP, noteType } from './model';

export type HomeTypeFilter =
	| 'all'
	| 'notes'
	| 'documents'
	| 'md'
	| 'tex'
	| 'ipynb'
	| 'pdf'
	| 'epub';

export function attachedDocumentIds(notes: NoteSummary[]) {
	const referenced = new Set(
		notes.map((note) => note.sourcePdf).filter((id): id is string => !!id)
	);
	const isCopyName = (note: NoteSummary) => {
		const name = note.relativePath.split(/[\\/]/).pop()?.toLowerCase() ?? '';
		return (
			/ \d+\.(pdf|epub)$/.test(name) ||
			/ [0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\.(pdf|epub)$/.test(name)
		);
	};
	return new Set(
		notes.filter((note) => referenced.has(note.id) && isCopyName(note)).map((note) => note.id)
	);
}

export function typeCounts(notes: NoteSummary[], attachedIds: Set<string>) {
	const counts: Record<string, number> = { md: 0, tex: 0, ipynb: 0, pdf: 0, epub: 0 };
	for (const note of notes) {
		if (DOC_GROUP.includes(noteType(note)) && attachedIds.has(note.id)) continue;
		counts[noteType(note)] += 1;
	}
	return counts;
}

export function filterNotes(
	notes: NoteSummary[],
	filter: HomeTypeFilter,
	tag: string | null,
	notebook: string | null,
	pinnedIds: string[],
	attachedIds: Set<string>
) {
	let result = notes;
	if (filter === 'notes') result = result.filter((note) => NOTE_GROUP.includes(noteType(note)));
	else if (filter === 'documents')
		result = result.filter(
			(note) => DOC_GROUP.includes(noteType(note)) && !attachedIds.has(note.id)
		);
	else if (filter !== 'all')
		result = result.filter((note) => noteType(note) === filter && !attachedIds.has(note.id));
	if (tag !== null) result = result.filter((note) => note.tags.includes(tag));
	if (notebook === null) result = result.filter((note) => notebookOf(note) === null);
	else
		result = result.filter(
			(note) => note.folder === notebook || note.folder.startsWith(notebook + '/')
		);
	return [...result].sort(
		(a, b) => (pinnedIds.includes(b.id) ? 1 : 0) - (pinnedIds.includes(a.id) ? 1 : 0)
	);
}

export function notebookOf(note: NoteSummary): string | null {
	if (!note.folder || note.folder === 'Root') return null;
	return note.folder.split('/')[0];
}

export function tagCounts(notes: NoteSummary[]) {
	const counts = new Map<string, number>();
	for (const note of notes) {
		for (const value of note.tags) {
			const tag = value.trim();
			if (tag) counts.set(tag, (counts.get(tag) ?? 0) + 1);
		}
	}
	return [...counts.entries()].sort((a, b) => b[1] - a[1]);
}

export function commonplaces(notes: NoteSummary[]) {
	const graph = new Map<string, Set<string>>();
	for (const note of notes) {
		if (!graph.has(note.id)) graph.set(note.id, new Set());
		for (const link of note.backlinks) {
			if (!graph.has(link.sourceId)) graph.set(link.sourceId, new Set());
			graph.get(note.id)!.add(link.sourceId);
			graph.get(link.sourceId)!.add(note.id);
		}
	}
	const visited = new Set<string>();
	const clusters: NoteSummary[][] = [];
	const noteMap = new Map(notes.map((note) => [note.id, note]));
	for (const note of notes) {
		if (visited.has(note.id)) continue;
		const cluster: string[] = [];
		const queue = [note.id];
		visited.add(note.id);
		while (queue.length) {
			const current = queue.shift()!;
			cluster.push(current);
			graph.get(current)?.forEach((neighbor) => {
				if (!visited.has(neighbor)) {
					visited.add(neighbor);
					queue.push(neighbor);
				}
			});
		}
		if (cluster.length > 1) {
			const mapped = cluster
				.map((id) => noteMap.get(id))
				.filter((value): value is NoteSummary => !!value);
			if (mapped.length > 1) clusters.push(mapped);
		}
	}
	return clusters.sort((a, b) => b.length - a.length);
}
