export type StructuralTextItem = { str?: string; transform?: number[]; height?: number; width?: number };

/** Preserve lines and paragraph gaps from PDF.js without rewriting source text. */
export function pdfItemsToStructuralText(items: StructuralTextItem[]): string {
	const positioned = items.map((item) => ({ text: typeof item.str === 'string' ? item.str : '', x: item.transform?.[4] ?? 0, y: item.transform?.[5] ?? 0, width: Math.abs(item.width ?? 0), height: Math.abs(item.height ?? item.transform?.[3] ?? 10) })).filter((item) => item.text);
	if (!positioned.length) return '';
	const heights = positioned.map((item) => item.height).sort((a, b) => a - b);
	const median = heights[Math.floor(heights.length / 2)] || 10;
	positioned.sort((a, b) => b.y - a.y || a.x - b.x);
	const lines: typeof positioned[] = [];
	for (const item of positioned) {
		const line = lines.find((candidate) => Math.abs(candidate[0].y - item.y) <= median * 0.45);
		if (line) line.push(item); else lines.push([item]);
	}
	// A stable central gutter with substantial text on both sides is read as
	// columns instead of globally interleaving equal-Y lines.
	const minX = Math.min(...positioned.map((item) => item.x));
	const maxX = Math.max(...positioned.map((item) => item.x + item.width));
	const mid = (minX + maxX) / 2;
	const left = lines.filter((line) => Math.max(...line.map((item) => item.x + item.width)) < mid - median);
	const right = lines.filter((line) => Math.min(...line.map((item) => item.x)) > mid + median);
	const full = lines.filter((line) => !left.includes(line) && !right.includes(line));
	const columnMode = left.length >= 3 && right.length >= 3 &&
		left.reduce((sum, line) => sum + line.length, 0) >= positioned.length * 0.25 &&
		right.reduce((sum, line) => sum + line.length, 0) >= positioned.length * 0.25;
	const fullBefore = full.filter((line) => line[0].y > Math.max(left[0]?.[0].y ?? -Infinity, right[0]?.[0].y ?? -Infinity));
	const fullAfter = full.filter((line) => line[0].y <= Math.max(left[0]?.[0].y ?? -Infinity, right[0]?.[0].y ?? -Infinity));
	const ordered = columnMode ? [...fullBefore, ...left, ...right, ...fullAfter] : lines;
	let previousY: number | undefined;
	return ordered.map((line) => {
		line.sort((a, b) => a.x - b.x);
		const gap = previousY === undefined ? 0 : previousY - line[0].y;
		previousY = line[0].y;
		const gaps = line.slice(1).map((item, index) => item.x - (line[index].x + line[index].width));
		const tableLike = gaps.filter((gap) => gap > median * 2).length >= 2 && line.length >= 3;
		const text = line.map((item, index) => {
			const prior = line[index - 1];
			const gapText = prior && item.x - (prior.x + prior.width) > Math.max(1, median * 0.12) ? (tableLike ? ' | ' : ' ') : '';
			return `${gapText}${item.text}`;
		}).join('').trim();
		return `${gap > median * 1.7 ? '\n' : ''}${text}`;
	}).join('\n').trim();
}

/** Remove repeated page furniture without touching page markers or body text. */
export function suppressRepeatedPageFurniture(pages: string[]): string[] {
	if (pages.length < 2) return pages;
	const candidates = new Map<string, number>();
	const edge = (page: string) => page.split('\n').map((line) => line.trim()).filter(Boolean).slice(0, 2).concat(page.split('\n').map((line) => line.trim()).filter(Boolean).slice(-2));
	for (const page of pages) for (const line of edge(page)) {
		const normalized = line.replace(/\d+/g, '#').replace(/\s+/g, ' ').toLowerCase();
		if (normalized.length >= 3) candidates.set(normalized, (candidates.get(normalized) ?? 0) + 1);
	}
	const repeated = new Set([...candidates.entries()].filter(([, count]) => count / pages.length >= 0.6).map(([line]) => line));
	return pages.map((page) => page.split('\n').filter((line, index, lines) => {
		const normalized = line.trim().replace(/\d+/g, '#').replace(/\s+/g, ' ').toLowerCase();
		const isEdge = index < 3 || index >= lines.length - 3;
		return !(isEdge && repeated.has(normalized));
	}).join('\n').trim());
}

export function htmlElementToStructuralText(root: Element): string {
	const clone = root.cloneNode(true) as Element;
	clone.querySelectorAll('script, style, nav, footer, form, [hidden]').forEach((node) => node.remove());
	return Array.from(clone.querySelectorAll('h1,h2,h3,h4,h5,h6,p,li,pre,blockquote,tr'))
		.map((node) => node.textContent?.replace(/\s+/g, ' ').trim() ?? '').filter(Boolean).join('\n\n');
}
