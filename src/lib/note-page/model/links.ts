import type { BlockItem } from '../types';

export function parseBlocks(markdown: string, sourceNoteId = '', sourceTitle = ''): BlockItem[] {
	const blocks: Array<BlockItem | null> = markdown.split(/\n+/).map((chunk) => {
		const original = chunk.trim();
		if (!original) return null;
		const idMatch = original.match(/\(\(([a-fA-F0-9]{6})\)\)$/);
		let text = original.replace(/\s*\(\([a-fA-F0-9]{6}\)\)$/, '');
		text = text.replace(/\[([^\]]+)\]\([^)]+\)/g, '$1');
		text = text.replace(/(\*\*|__)(.*?)\1/g, '$2');
		text = text.replace(/(\*|_)(.*?)\1/g, '$2');
		text = text.replace(/^#+\s+/g, '');
		return {
			id: idMatch?.[1] ?? null,
			original,
			text,
			sourceNoteId,
			sourceNoteTitle: sourceTitle
		};
	});
	return blocks.filter((block): block is BlockItem => block !== null);
}

export function transclusionTarget(href: string): { noteId: string; blockId: string } | null {
	const match = href.match(/\/notes\/([^#]+)#([a-fA-F0-9]{6})$/);
	if (!match) return null;
	return { noteId: decodeURIComponent(match[1]), blockId: match[2] };
}
