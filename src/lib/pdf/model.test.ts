import { describe, expect, it } from 'vitest';
import { clampPage, eraserHit, fitWidthScale, normalizedRect, pageOrder } from './model';

describe('pdf model', () => {
	it('clamps pages and keeps the active page first', () => {
		expect(clampPage(-2, 4)).toBe(1);
		expect(clampPage(9, 4)).toBe(4);
		expect(pageOrder(3, 4)).toEqual([3, 1, 2, 4]);
	});

	it('handles empty geometry safely', () => {
		expect(clampPage(1, 0)).toBe(0);
		expect(fitWidthScale(0, 600, false)).toBe(0);
		expect(normalizedRect(8, 10, 2, 4)).toEqual([2, 4, 6, 6]);
		expect(
			eraserHit(
				[3, 0],
				[
					[0, 0],
					[10, 0]
				],
				3
			)
		).toBe(true);
	});
});
