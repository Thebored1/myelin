export type ScrollMode = 'vertical' | 'horizontal' | 'wrapped' | 'page';
export type SpreadMode = 'none' | 'odd' | 'even';
export type ToolMode = 'select' | 'pen' | 'eraser' | 'marquee';
export type PdfPoint = [number, number];
export type SelectionRect = [number, number, number, number];

export type PdfSection = { key: string; label: string; content: string };

import type { PdfAnnotation } from '$lib/types';

export type PdfViewerProps = {
	pdfBytes: Uint8Array;
	annotations?: PdfAnnotation[];
	onQuote?: (quote: string, pageNum: number, rects?: { x: number; y: number; width: number; height: number }[]) => void;
	onAnnotationsChange?: (annotations: PdfAnnotation[]) => void;
	onImageExtract?: (base64: string) => void;
	onTextExtracted?: (text: string) => void;
	onActiveSection?: (section: PdfSection) => void;
	onSectionsReady?: (sections: PdfSection[]) => void;
	onAttachNote?: () => void;
	onClosePdf?: () => void;
	showAttachButton?: boolean;
};
