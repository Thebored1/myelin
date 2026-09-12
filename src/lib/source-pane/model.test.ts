import { describe, expect, it } from 'vitest';
import { distanceToSegment, inkHit, normalizedRect } from './model';

describe('source pane model', () => {
	it('computes normalized rects regardless of drag direction', () => {
		expect(normalizedRect(10, 20, 5, 8)).toEqual({ x: 5, y: 8, width: 5, height: 12 });
	});

	it('hits ink strokes within radius and misses outside', () => {
		const path: [number, number][] = [
			[0, 0],
			[10, 0]
		];
		expect(inkHit([5, 1], path, 2)).toBe(true);
		expect(inkHit([5, 5], path, 2)).toBe(false);
		expect(inkHit([0, 0], [[5, 5]], 8)).toBe(true);
		expect(inkHit([0, 0], [[5, 5]], 2)).toBe(false);
	});

	it('computes point-to-segment distance', () => {
		expect(distanceToSegment([5, 3], [0, 0], [10, 0])).toBe(3);
	});

	it('resolves offsets across segments', async () => {
		// segmentRange is DOM-free; fake the Text nodes with tagged objects.
		const fake = (data: string) => ({ data }) as unknown as Text;
		const { segmentRange } = await import('./model');
		const index = {
			segments: [
				{ node: fake('Hello '), start: 0, end: 6 },
				{ node: fake('world'), start: 6, end: 11 }
			],
			length: 11
		};
		expect(segmentRange(index, 0, 5)?.startOffset).toBe(0);
		expect(segmentRange(index, 4, 8)?.startNode).toBe(index.segments[0].node);
		expect(segmentRange(index, 4, 8)?.endNode).toBe(index.segments[1].node);
		expect(segmentRange(index, 4, 8)?.endOffset).toBe(2);
		expect(segmentRange(index, 5, 5)).toBeNull();
	});
});
