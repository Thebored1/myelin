/**
 * Geometry contract for the note-page right sidebar.
 *
 * The chat composer is a single non-wrapping flex row (`.prompt-toolbar`):
 * attach button, Chat/Write group, Allow toggle, the selection pill, a
 * flexible spacer, and the send button. Nothing in that row wraps, and the
 * panel clips its overflow, so the rightmost items — the selection pill and
 * send button — disappear first when the sidebar is dragged narrow.
 *
 * These numbers are measured from the shipped stylesheet rather than guessed:
 * `.sidebar` supplies `--space-6` horizontal padding on each side, the mode
 * groups supply 2px padding + 1px border, `.prompt-toolbar` supplies 2px
 * padding with 5px gaps, and the sidebar sets `font-family: var(--font-mono)`
 * so text advances at roughly 0.6em. `minSidebarWidth` returns a width that
 * clears the widest realistic row.
 */

/** `--space-6`, the sidebar's horizontal padding on each side. */
const SIDEBAR_PADDING_X = 1.5 * 16;
/** `.prompt-toolbar` gap between children. */
const TOOLBAR_GAP = 5;
/** `.prompt-toolbar` vertical padding, applied horizontally too. */
const TOOLBAR_PADDING_X = 2 * 2;
/** `.prompt-icon-btn` and `.send-btn` are fixed squares. */
const ICON_BUTTON_WIDTH = 28;
/** Average glyph advance of JetBrains Mono, as a fraction of the font size. */
const MONO_ADVANCE = 0.6;

/** Text width in the sidebar's mono face, at `rem` with `padding` per side. */
function monoWidth(text: string, rem: number, padding = 0): number {
	return text.length * rem * 16 * MONO_ADVANCE + padding * 2;
}

/** A `.interaction-mode` group: buttons with 9px side padding, inside a bordered box. */
function interactionModeWidth(labels: string[]): number {
	const buttons = labels.reduce((sum, label) => sum + monoWidth(label, 0.68, 9), 0);
	return buttons + 2 * 2 + 2 * 1;
}

/**
 * The `.selection-pill`: 9px left / 5px right padding, a 13px target icon, a
 * 6px gap, the label, another 6px gap, and the 16px dismiss dot.
 *
 * The label truncates with an ellipsis, so the pill contributes its fixed
 * chrome plus however much label is actually available; only this minimum
 * has to fit for the row to stay intact.
 */
function selectionPillChrome(): number {
	return 9 + 13 + 6 + 6 + 16 + 5;
}

/** A single glyph, the narrowest label that still reads as elided text. */
const ELIDED_LABEL_WIDTH = monoWidth('…', 0.72);

/** Narrowest the pill can get while staying usable. */
function selectionPillMinWidth(): number {
	return selectionPillChrome() + ELIDED_LABEL_WIDTH;
}

/** Full, untruncated width of the pill for a given label. */
function selectionPillWidth(label: string): number {
	return selectionPillChrome() + monoWidth(label, 0.72);
}

/** Width of everything in the composer row except the selection pill. */
function fixedRowWidth(): number {
	const children = [
		ICON_BUTTON_WIDTH,
		interactionModeWidth(['Chat', 'Write']),
		interactionModeWidth(['Allow']),
		ICON_BUTTON_WIDTH
	];
	const row = children.reduce((sum, width) => sum + width, 0);
	const gaps = TOOLBAR_GAP * (children.length - 1);
	return row + gaps + TOOLBAR_PADDING_X + SIDEBAR_PADDING_X * 2;
}

/**
 * Width the sidebar must have so the composer row fits *at its worst* — the
 * selection pill fully ellipsized. This is the floor that keeps the send
 * button and every mode toggle reachable.
 */
export function composerMinWidth(): number {
	const children = [selectionPillMinWidth()];
	// Five gaps: attach | mode | allow | pill | spacer | send
	return fixedRowWidth() + children[0] + TOOLBAR_GAP * 5;
}

/**
 * Width the row would occupy with a given label shown in full. Useful for
 * sizing the default; the sidebar clamps to `SIDEBAR_MIN_WIDTH` regardless.
 */
export function composerIntrinsicWidth(selectionLabel: string | null): number {
	if (selectionLabel === null) return fixedRowWidth();
	return fixedRowWidth() + selectionPillWidth(selectionLabel) + TOOLBAR_GAP;
}

/** Label used for a notebook cell write target, e.g. `Cell 12 · Cursor`. */
export function cellSelectionLabel(cellIndex: number, target: string): string {
	return `Cell ${cellIndex} · ${target}`;
}

/**
 * Minimum draggable/usable sidebar width. Sized to the widest realistic
 * composer row so no control is ever clipped at the minimum.
 */
export const SIDEBAR_MIN_WIDTH = 400;
