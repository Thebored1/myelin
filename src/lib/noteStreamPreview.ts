export type NoteStreamTarget = {
	text: string;
	before: string;
	after: string;
	cursor: boolean;
	sourceOffset?: number;
	lineBreaks?: number;
};

export type NoteStreamPreviewResult = {
	preview: string;
	applied: boolean;
};

/** Remove only model protocol wrappers from the speculative editor preview. */
function cleanGeneratedPreview(value: string): string {
	let cleaned = value;
	const wrapper = cleaned.match(/^\s*\{\s*["']?text["']?\s*:\s*["']?/);
	if (wrapper) {
		cleaned = cleaned.slice(wrapper[0].length);
		cleaned = cleaned.replace(/["']\s*}\s*$/, '');
	}
	const markers = [
		'\n{text:',
		'\n' + String.fromCharCode(96, 96, 96),
		'</text>',
		'>>/text>',
		'>/text>',
		'</content>',
		'</tool_call>'
	];
	const cut = markers
		.map((marker) => cleaned.indexOf(marker))
		.filter((index) => index >= 0)
		.sort((left, right) => left - right)[0];
	if (cut !== undefined) cleaned = cleaned.slice(0, cut);
	return cleaned.replace(/\s*[—-]?\s*Myelin,\s*focused editor\s*>{1,}/gi, '\n');
}

function normalizeAnchor(value: string): string {
	return value.trim().replace(/\s+/g, ' ');
}

function tolerantMatches(source: string, anchor: string): Array<[number, number]> {
	const trimmed = anchor.trim();
	if (!trimmed) return [];
	const tokens = trimmed.split(/\s+/).map((token) => token.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'));
	const pattern = new RegExp(tokens.join('\\s+'), 'g');
	return Array.from(source.matchAll(pattern), (match) => [
		match.index ?? 0,
		(match.index ?? 0) + match[0].length
	]);
}

function normalizedStartsWith(value: string, anchor: string): boolean {
	return Boolean(anchor) && normalizeAnchor(value).startsWith(anchor);
}

function normalizedEndsWith(value: string, anchor: string): boolean {
	return Boolean(anchor) && normalizeAnchor(value).endsWith(anchor);
}

function cursorAnchorMatches(
	source: string,
	anchor: string,
	fromEnd: boolean
): Array<[number, number]> {
	const matches = tolerantMatches(source, anchor);
	if (matches.length > 0) return matches;

	// Vditor IR removes Markdown punctuation from the rendered text. Fall back
	// to a short word anchor so the preview uses the same source boundary as the
	// native write tool when markers such as **bold** or `code` are present.
	const words = anchor.split(/[^\p{L}\p{N}]+/u).filter(Boolean);
	if (words.length === 0) return [];
	const selected = fromEnd ? words.slice(-8) : words.slice(0, 8);
	const pattern = selected
		.map((word) => word.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'))
		.join('[^\\p{L}\\p{N}]+');
	const regex = new RegExp(pattern, 'gu');
	return Array.from(source.matchAll(regex), (match) => [
		match.index ?? 0,
		(match.index ?? 0) + match[0].length
	]);
}

function isCursorSeparator(value: string): boolean {
	return !value || /^[\s*_`#>~\[\]{}():;,!?"'\-—–/.]+$/u.test(value);
}

function uniqueCursorGap(source: string, before: string, after: string): number | null {
	const beforeMatches = cursorAnchorMatches(source, before, true);
	const afterMatches = cursorAnchorMatches(source, after, false);
	const gaps: Array<[number, number]> = [];
	for (const [, end] of beforeMatches) {
		for (const [start] of afterMatches) {
			const gap = source.slice(end, start);
			if (end <= start && start - end <= 256 && isCursorSeparator(gap)) {
				const markerPrefix = gap.match(/^[^\s\p{L}\p{N}]*/u)?.[0].length ?? 0;
				gaps.push([start - end, end + markerPrefix]);
			}
		}
	}
	gaps.sort((left, right) => left[0] - right[0] || left[1] - right[1]);
	const unique = gaps.filter(
		([distance, position], index) =>
			index === 0 || distance !== gaps[index - 1][0] || position !== gaps[index - 1][1]
	);
	return unique.length === 1 && unique[0][0] <= 256 ? unique[0][1] : null;
}

function addCursorLineBreaks(source: string, position: number, generated: string, lineBreaks = 0): string {
	if (lineBreaks <= 0) return generated;
	const existingBreaks = (source.slice(0, position).match(/\n+$/)?.[0].length ?? 0);
	const generatedBreaks = generated.match(/^\n*/)?.[0].length ?? 0;
	const neededBreaks = Math.max(0, lineBreaks - existingBreaks - generatedBreaks);
	return '\n'.repeat(neededBreaks) + generated;
}

export function locateNoteStreamTarget(
	source: string,
	target: NoteStreamTarget
): [number, number] | null {
	if (target.cursor) {
		// The editor captures this offset from Vditor's own Markdown serializer,
		// so use it when the surrounding source is still unchanged. Anchors remain
		// the safe fallback if the note was edited while the request was pending.
		if (target.sourceOffset !== undefined && source.trim()) {
			const position = target.sourceOffset;
			const beforeMatches =
				!target.before || source.slice(0, position).endsWith(target.before);
			const afterMatches =
				!target.after || source.slice(position).startsWith(target.after);
			if (position >= 0 && position <= source.length && beforeMatches && afterMatches)
				return [position, position];
		}
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
		if (positions.length === 1) return [positions[0], positions[0]];

		// Rendered Markdown can omit hard-break spaces that exist in the source,
		// so exact anchors may disagree even though they still identify one cursor.
		const before = target.before.trim();
		const after = target.after.trim();
		const beforeNorm = normalizeAnchor(before);
		const afterNorm = normalizeAnchor(after);
		const candidates: Array<[number, number]> = [];
		for (const [, end] of tolerantMatches(source, before)) {
			let position = end;
			if (after && !source.startsWith(after, position)) {
				const whitespace =
					source.slice(position).length - source.slice(position).trimStart().length;
				if (normalizedStartsWith(source.slice(position), afterNorm)) position += whitespace;
			}
			const score = !after
				? 1
				: source.startsWith(after, position)
					? 3
					: normalizedStartsWith(source.slice(position), afterNorm)
						? 2
						: 0;
			if (score > 0) candidates.push([score, position]);
		}
		for (const [start] of tolerantMatches(source, after)) {
			const score = !before
				? 1
				: source.slice(0, start).endsWith(before)
					? 3
					: normalizedEndsWith(source.slice(0, start), beforeNorm)
						? 2
						: 0;
			if (score > 0) candidates.push([score, start]);
		}
		const bestScore = Math.max(...candidates.map(([score]) => score), 0);
		const best = [
			...new Set(
				candidates.filter(([score]) => score === bestScore).map(([, position]) => position)
			)
		];
		if (best.length === 1) return [best[0], best[0]];
		const beforeMatches = cursorAnchorMatches(source, before, true);
		if (beforeMatches.length === 1) {
			const [, end] = beforeMatches[0];
			const markerPrefix = source.slice(end).match(/^[^\s\p{L}\p{N}]*/u)?.[0].length ?? 0;
			return [end + markerPrefix, end + markerPrefix];
		}
		const gap = uniqueCursorGap(source, before, after);
		return gap === null ? null : [gap, gap];
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
	const cleanGenerated = cleanGeneratedPreview(generated);
	if (!target) return { preview: cleanGenerated, applied: true };
	const span = cachedSpan !== undefined ? cachedSpan : locateNoteStreamTarget(source, target);
	if (!span) return { preview: source, applied: false };
	return {
		preview:
			source.slice(0, span[0]) + addCursorLineBreaks(source, span[0], cleanGenerated, target.lineBreaks) + source.slice(span[1]),
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
