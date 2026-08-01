export type NoteStreamTarget = {
	text: string;
	before: string;
	after: string;
	cursor: boolean;
};

export type NoteStreamPreviewResult = {
	preview: string;
	applied: boolean;
};

export function locateNoteStreamTarget(
	source: string,
	target: NoteStreamTarget
): [number, number] | null {
	if (target.cursor) {
		const positions: number[] = [];
		if (target.before) {
			let from = 0;
			while (positions.length < 2) {
				const anchor = source.indexOf(target.before, from);
				if (anchor < 0) break;
				const position = anchor + target.before.length;
				if (!target.after || source.startsWith(target.after, position)) positions.push(position);
				from = anchor + 1;
			}
		} else if (target.after) {
			let from = 0;
			while (positions.length < 2) {
				const position = source.indexOf(target.after, from);
				if (position < 0) break;
				positions.push(position);
				from = position + 1;
			}
		} else if (!source.trim()) {
			// Editors commonly serialize a visually empty note as one or more
			// whitespace characters. Match the backend cursor-write behavior by
			// replacing that blank span instead of inserting beside it.
			return [0, source.length];
		}
		return positions.length === 1 ? [positions[0], positions[0]] : null;
	}

	let best: { score: number; start: number; end: number } | null = null;
	let from = 0;
	while (target.text && from <= source.length) {
		const start = source.indexOf(target.text, from);
		if (start < 0) break;
		const end = start + target.text.length;
		const beforeMatches = !target.before || source.slice(0, start).endsWith(target.before);
		const afterMatches = !target.after || source.startsWith(target.after, end);
		const score = Number(beforeMatches) + Number(afterMatches);
		if (!best || score > best.score) best = { score, start, end };
		if (score === 2) break;
		from = start + 1;
	}
	return best ? [best.start, best.end] : null;
}

export function composeNoteStreamPreviewWithStatus(
	source: string,
	generated: string,
	target: NoteStreamTarget | null,
	cachedSpan?: [number, number] | null
): NoteStreamPreviewResult {
	if (!target) return { preview: generated, applied: true };
	const span = cachedSpan !== undefined ? cachedSpan : locateNoteStreamTarget(source, target);
	if (!span) return { preview: source, applied: false };
	return {
		preview: source.slice(0, span[0]) + generated + source.slice(span[1]),
		applied: true
	};
}

export function composeNoteStreamPreview(
	source: string,
	generated: string,
	target: NoteStreamTarget | null
): string {
	return composeNoteStreamPreviewWithStatus(source, generated, target).preview;
}
