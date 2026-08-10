import type { BlockItem } from '../types';

export function parseBlocks(markdown: string, sourceNoteId = '', sourceTitle = ''): BlockItem[] {
	return markdown
		.split(/\n+/)
		.map((chunk) => chunk.trim())
		.filter(Boolean)
		.map((original) => {
			const idMatch = original.match(/\(\(([a-fA-F0-9]{6})\)\)$/);
			const id = idMatch?.[1] ?? '';
			return {
				id,
				original,
				text: original.replace(/\s*\(\([a-fA-F0-9]{6}\)\)$/, ''),
				sourceNoteId,
				sourceTitle
			};
		});
}

export function transclusionTarget(href: string): { noteId: string; blockId: string } | null {
	const match = href.match(/\/notes\/([^#]+)#([a-fA-F0-9]{6})$/);
	if (!match) return null;
	return { noteId: decodeURIComponent(match[1]), blockId: match[2] };
}

