export type SourceToolMode = 'select' | 'pen' | 'eraser' | 'marquee';

/** Re-exported for convenience: kinds the unified pane renders. */
export type { SourceKind, SourceAnnotation } from '$lib/types';

export type PaneTextSegment = {
	node: Text;
	start: number;
	end: number;
};

/** Maps character offsets in the pane's plain text to DOM text nodes. */
export type PaneTextIndex = {
	segments: PaneTextSegment[];
	length: number;
};

export type SelectionAnchor = {
	start: number;
	end: number;
	quote: string;
};

/** Rects of a highlight in overlay (scroll-size) coordinates. */
export type OverlayRect = { x: number; y: number; width: number; height: number };
