import type { PdfPoint, SelectionRect } from './types';

export function clampPage(page: number, pageCount: number): number {
	if (pageCount <= 0) return 0;
	return Math.min(pageCount, Math.max(1, Math.round(page)));
}

export function pageOrder(activePage: number, pageCount: number): number[] {
	if (pageCount <= 0) return [];
	const active = clampPage(activePage, pageCount) || 1;
	return [active, ...Array.from({ length: pageCount }, (_, index) => index + 1).filter((page) => page !== active)];
}

export function fitWidthScale(viewportWidth: number, paneWidth: number, spread: boolean): number {
	if (viewportWidth <= 0 || paneWidth <= 0) return 0;
	const scrollbar = 18;
	const reserved = spread ? 96 : 64;
	return (paneWidth - reserved - scrollbar) / (viewportWidth * (spread ? 2 : 1));
}

export function normalizedRect(startX: number, startY: number, endX: number, endY: number): SelectionRect {
	return [Math.min(startX, endX), Math.min(startY, endY), Math.abs(endX - startX), Math.abs(endY - startY)];
}

export function distanceToSegment(point: PdfPoint, start: PdfPoint, end: PdfPoint): number {
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

export function eraserHit(point: PdfPoint, path: PdfPoint[], radius: number): boolean {
	if (path.length === 0) return false;
	if (path.length === 1) return Math.hypot(point[0] - path[0][0], point[1] - path[0][1]) <= radius;
	return path.slice(1).some((end, index) => distanceToSegment(point, path[index], end) <= radius);
}
