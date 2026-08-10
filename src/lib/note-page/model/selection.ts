import type { AiEditTarget } from '../types';

/** Find the occurrence nearest to a previously captured editor offset. */
export function nearestIndexOf(haystack: string, needle: string, hint: number): number {
	if (!needle) return -1;
	let best = -1;
	let bestDistance = Infinity;
	let from = 0;
	while (from <= haystack.length) {
		const index = haystack.indexOf(needle, from);
		if (index < 0) break;
		const distance = Math.abs(index - hint);
		if (distance < bestDistance) {
			best = index;
			bestDistance = distance;
		}
		from = index + 1;
	}
	return best;
}

export function selectionTarget(
	text: string,
	start: number,
	end: number,
	context = 40
): AiEditTarget {
	const safeStart = Math.max(0, Math.min(start, text.length));
	const safeEnd = Math.max(safeStart, Math.min(end, text.length));
	return {
		text: text.slice(safeStart, safeEnd),
		before: text.slice(Math.max(0, safeStart - context), safeStart),
		after: text.slice(safeEnd, Math.min(text.length, safeEnd + context)),
		start: safeStart,
		end: safeEnd
	};
}

export function cursorTarget(text: string, position: number, context = 80): AiEditTarget {
	const safePosition = Math.max(0, Math.min(position, text.length));
	return {
		text: '',
		before: text.slice(Math.max(0, safePosition - context), safePosition),
		after: text.slice(safePosition, Math.min(text.length, safePosition + context)),
		start: safePosition,
		end: safePosition,
		cursor: true
	};
}

