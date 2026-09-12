import type { PaneTextIndex, SelectionAnchor } from './types';

/** Distance from point to segment — shared eraser hit-testing math. */
export function distanceToSegment(
	point: [number, number],
	start: [number, number],
	end: [number, number]
): number {
	const [px, py] = point;
	const [sx, sy] = start;
	const [ex, ey] = end;
	const dx = ex - sx;
	const dy = ey - sy;
	const lengthSquared = dx * dx + dy * dy;
	if (lengthSquared === 0) return Math.hypot(px - sx, py - sy);
	const t = Math.max(0, Math.min(1, ((px - sx) * dx + (py - sy) * dy) / lengthSquared));
	return Math.hypot(px - (sx + t * dx), py - (sy + t * dy));
}

/** True when the eraser point falls within radius of an ink stroke. */
export function inkHit(point: [number, number], path: [number, number][], radius: number): boolean {
	if (path.length === 0) return false;
	if (path.length === 1) return Math.hypot(point[0] - path[0][0], point[1] - path[0][1]) <= radius;
	return path.slice(1).some((end, index) => distanceToSegment(point, path[index], end) <= radius);
}

export function normalizedRect(
	startX: number,
	startY: number,
	endX: number,
	endY: number
): { x: number; y: number; width: number; height: number } {
	return {
		x: Math.min(startX, endX),
		y: Math.min(startY, endY),
		width: Math.abs(endX - startX),
		height: Math.abs(endY - startY)
	};
}

/**
 * Walks an element's text nodes once and records each node's span in the
 * pane-wide character space. Highlights anchor to offsets in this space so
 * they survive re-render and work for every text-bearing format alike.
 */
export function buildTextIndex(root: Element): PaneTextIndex {
	const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
	const segments: PaneTextIndex['segments'] = [];
	let cursor = 0;
	let node = walker.nextNode() as Text | null;
	while (node) {
		const length = node.data.length;
		if (length > 0) segments.push({ node, start: cursor, end: cursor + length });
		cursor += length;
		node = walker.nextNode() as Text | null;
	}
	return { segments, length: cursor };
}

export function segmentRange(
	index: PaneTextIndex,
	start: number,
	end: number
): { startNode: Text; startOffset: number; endNode: Text; endOffset: number } | null {
	if (index.length === 0 || end <= start) return null;
	let startNode: Text | null = null;
	let startOffset = 0;
	let endNode: Text | null = null;
	let endOffset = 0;
	for (const segment of index.segments) {
		if (!startNode && start < segment.end) {
			startNode = segment.node;
			startOffset = Math.max(0, start - segment.start);
		}
		if (start >= segment.start && start < segment.end) {
			startNode = segment.node;
			startOffset = start - segment.start;
		}
		if (end > segment.start && end <= segment.end) {
			endNode = segment.node;
			endOffset = end - segment.start;
			break;
		}
	}
	if (!startNode || !endNode) return null;
	return { startNode, startOffset, endNode, endOffset };
}

/** Resolve character offsets back to a live DOM range inside the pane. */
export function resolveRange(index: PaneTextIndex, start: number, end: number): Range | null {
	const bounds = segmentRange(index, Math.max(0, start), Math.min(index.length, end));
	if (!bounds) return null;
	const range = document.createRange();
	range.setStart(bounds.startNode, bounds.startOffset);
	range.setEnd(bounds.endNode, bounds.endOffset);
	return range;
}

/** Client rects of a character range, translated into overlay space. */
export function rectsForRange(
	index: PaneTextIndex,
	start: number,
	end: number,
	content: HTMLElement
): { x: number; y: number; width: number; height: number }[] {
	const range = resolveRange(index, start, end);
	if (!range) return [];
	const origin = content.getBoundingClientRect();
	return Array.from(range.getClientRects())
		.filter((rect) => rect.width > 0 && rect.height > 0)
		.map((rect) => ({
			x: rect.left - origin.left + content.scrollLeft,
			y: rect.top - origin.top + content.scrollTop,
			width: rect.width,
			height: rect.height
		}));
}

/** Serialize the current DOM selection inside root to character offsets. */
export function serializeSelection(root: Element, selection: Selection): SelectionAnchor | null {
	if (selection.rangeCount === 0 || selection.isCollapsed) return null;
	const range = selection.getRangeAt(0);
	if (!root.contains(range.commonAncestorContainer)) return null;
	const pre = document.createRange();
	pre.selectNodeContents(root);
	try {
		pre.setEnd(range.startContainer, range.startOffset);
	} catch {
		return null;
	}
	const start = pre.toString().length;
	const end = start + range.toString().length;
	const quote = range.toString();
	if (end <= start) return null;
	return { start, end, quote };
}
