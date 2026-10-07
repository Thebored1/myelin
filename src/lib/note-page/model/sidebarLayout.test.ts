import { describe, expect, it } from 'vitest';
import {
	SIDEBAR_MIN_WIDTH,
	cellSelectionLabel,
	composerIntrinsicWidth,
	composerMinWidth
} from './sidebarLayout';

describe('note page sidebar layout', () => {
	it('fits the composer row with no selection pill', () => {
		expect(composerMinWidth()).toBeLessThan(SIDEBAR_MIN_WIDTH);
	});

	it('fits the row when the selection pill is fully ellipsized', () => {
		// The pill truncates (see .selection-pill in note-page.css), so the row
		// only has to clear the pill's fixed chrome plus one elided glyph.
		expect(composerMinWidth()).toBeLessThan(SIDEBAR_MIN_WIDTH);
	});

	it('still fits common targets at full width', () => {
		expect(composerIntrinsicWidth('Cursor')).toBeLessThan(SIDEBAR_MIN_WIDTH);
		expect(composerIntrinsicWidth('42 sel')).toBeLessThan(SIDEBAR_MIN_WIDTH);
	});

	it('reports a notebook cell target as wider than the plain label', () => {
		const cell = cellSelectionLabel(12, 'Cursor');
		expect(cell).toBe('Cell 12 · Cursor');
		expect(composerIntrinsicWidth(cell)).toBeGreaterThan(composerIntrinsicWidth('Cursor'));
		// Long cell targets exceed the minimum, which is exactly why the pill
		// ellipsizes instead of pushing the send button out of the panel.
		expect(composerIntrinsicWidth(cell)).toBeGreaterThan(SIDEBAR_MIN_WIDTH);
	});

	it('keeps the minimum above the row it must contain', () => {
		// Guards the specific regression: at the old 320px minimum the selection
		// pill and send button were clipped by the panel edge.
		expect(SIDEBAR_MIN_WIDTH).toBeGreaterThanOrEqual(380);
		expect(SIDEBAR_MIN_WIDTH).toBeGreaterThan(composerMinWidth());
	});
});
