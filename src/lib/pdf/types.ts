export type ScrollMode = 'vertical' | 'horizontal' | 'wrapped' | 'page';
export type SpreadMode = 'none' | 'odd' | 'even';
export type ToolMode = 'select' | 'pen' | 'eraser' | 'marquee';
export type PdfPoint = [number, number];
export type SelectionRect = [number, number, number, number];

export type PdfSection = { key: string; label: string; content: string };
