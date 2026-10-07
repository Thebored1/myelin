import type { PdfSection } from '$lib/pdf/types';

/**
 * Markdown sectioning for the section-scoped KV cache.
 *
 * ## Why this is gated on note size
 *
 * A KV slot is not an embedding: it is llama.cpp's internal attention state for
 * one exact prompt prefix, so it can only be produced by evaluating that prefix
 * (~95 tok/s on CPU). Embedding a chunk once and reusing it — what the RAG
 * index does — is a different mechanism and cannot stand in for a slot.
 *
 * Priming every section of every note would therefore be expensive per open.
 * Two rules keep that cost proportional to what the reader actually touches:
 *
 *  1. A note that fits in context is never sectioned. It goes in whole, with no
 *     slot traffic at all. The threshold mirrors `NotePromptShape::build` in
 *     `src-tauri/src/note_prompt.rs` exactly, so both sides agree on what
 *     "oversized" means.
 *  2. Sections are chunk-sized rather than heading-sized, so one section costs
 *     a few seconds to prime instead of tens of seconds.
 *
 * ## Keys
 *
 * The backend keys the on-disk slot on `(noteId, section.key, excerpt,
 * identity)`. Keys are therefore derived from heading text plus an occurrence
 * counter rather than from position: inserting a heading above must not
 * invalidate every section below it, while editing a section must invalidate
 * that section.
 */

/** Longest label kept for a section header. */
const MAX_LABEL = 120;
/** Fence openers recognised when skipping code blocks. */
const FENCE = /^\s{0,3}(`{3,}|~{3,})/;
/** ATX heading: one to six hashes, whitespace, then the title. */
const ATX_HEADING = /^\s{0,3}(#{1,6})\s+(.*?)\s*#*\s*$/;

/**
 * Chunk geometry, mirroring `embeddings.rs` (256-token target, 40-token
 * overlap, 384-token hard maximum) converted with the same ~4-chars-per-token
 * estimate the rest of the pipeline uses. Sections and RAG chunks then land on
 * comparable boundaries.
 */
const CHARS_PER_TOKEN = 4;
const CHUNK_TARGET_CHARS = 256 * CHARS_PER_TOKEN;
const CHUNK_MAX_CHARS = 384 * CHARS_PER_TOKEN;
const CHUNK_OVERLAP_CHARS = 40 * CHARS_PER_TOKEN;

/** Clamp floor/ceiling used by the backend's oversized test. */
const OVERSIZED_MIN_CHARS = 4_000;
const OVERSIZED_MAX_CHARS = 400_000;

/** A heading-delimited block, before any chunk splitting. */
export type SectionBlock = { label: string; content: string; headingIndex: number };

/** A section handed to the backend, plus the heading it was derived from. */
export type MappedSection = PdfSection & {
	/** Index of the rendered heading element this section belongs to. */
	headingIndex: number;
	/** Fraction of the owning block where this section starts, for scroll use. */
	startFraction: number;
	endFraction: number;
};

/**
 * Character budget above which a note is treated as oversized.
 *
 * Mirrors `NotePromptShape::build`: `ctx * 2` characters, clamped. Kept in
 * lockstep with the Rust side so the frontend never sections a note the
 * backend considers small (or vice versa).
 */
export function oversizedCharLimit(contextTokens: number): number {
	const raw = Math.trunc(contextTokens) * 2;
	return Math.min(Math.max(raw, OVERSIZED_MIN_CHARS), OVERSIZED_MAX_CHARS);
}

/**
 * Whether a note is large enough to be worth sectioning. Small notes are sent
 * whole and must not trigger slot preparation.
 */
export function shouldSectionNote(charCount: number, contextTokens: number): boolean {
	return charCount > oversizedCharLimit(contextTokens);
}

/** Lowercase, punctuation-free slug used in the cache key. */
function slug(label: string): string {
	const cleaned = label
		.toLowerCase()
		.replace(/[^\p{Letter}\p{Number}]+/gu, '-')
		.replace(/^-+|-+$/g, '')
		.slice(0, 48);
	return cleaned || 'section';
}

/**
 * Split a Markdown body into heading-delimited blocks.
 *
 * Text before the first heading is folded into that heading's block, which
 * keeps one block per rendered heading. Headings inside fenced code are
 * ignored, so the block count matches the editor's heading elements. Returns
 * an empty array when the body has no headings — a headingless note has no
 * "currently visible" unit, so it falls back to the RAG path.
 */
export function parseMarkdownBlocks(body: string): SectionBlock[] {
	if (!body.trim()) return [];

	const lines = body.split(/\r?\n/);
	const headings: { label: string; line: number }[] = [];
	let fence: string | null = null;

	for (let i = 0; i < lines.length; i++) {
		const fenceMatch = FENCE.exec(lines[i]);
		if (fenceMatch) {
			if (fence === null) fence = fenceMatch[1];
			else if (fenceMatch[1][0] === fence[0] && fenceMatch[1].length >= fence.length) fence = null;
			continue;
		}
		if (fence !== null) continue;
		const heading = ATX_HEADING.exec(lines[i]);
		if (!heading) continue;
		const label = heading[2].trim();
		if (label) headings.push({ label, line: i });
	}
	if (headings.length === 0) return [];

	return headings.map((heading, index) => {
		const end = headings[index + 1]?.line ?? lines.length;
		// The preamble joins the first heading's block so block count stays equal
		// to rendered heading count.
		const start = index === 0 ? 0 : heading.line;
		return {
			label: heading.label,
			content: lines.slice(start, end).join('\n').trim(),
			headingIndex: index
		};
	});
}

/**
 * Split one block into chunk-sized pieces on paragraph boundaries, falling back
 * to a hard character split when a single paragraph exceeds the maximum.
 * Overlap carries a little context across the seam.
 */
function chunkBlock(block: SectionBlock): string[] {
	if (block.content.length <= CHUNK_MAX_CHARS) return [block.content];

	const pieces: string[] = [];
	const paragraphs = block.content.split(/\n{2,}/);
	let current = '';
	const flush = () => {
		if (current.trim()) pieces.push(current.trim());
		current = '';
	};

	for (const paragraph of paragraphs) {
		if (paragraph.length > CHUNK_MAX_CHARS) {
			flush();
			for (let at = 0; at < paragraph.length; at += CHUNK_TARGET_CHARS) {
				pieces.push(paragraph.slice(at, at + CHUNK_MAX_CHARS));
			}
			continue;
		}
		const candidate = current ? `${current}\n\n${paragraph}` : paragraph;
		if (candidate.length > CHUNK_TARGET_CHARS) {
			// Carry the tail of the previous chunk so a sentence spanning the seam
			// is not lost from both sides.
			const tail = CHUNK_OVERLAP_CHARS > 0 ? current.slice(-CHUNK_OVERLAP_CHARS) : '';
			flush();
			current = tail ? `${tail}\n\n${paragraph}` : paragraph;
		} else {
			current = candidate;
		}
	}
	flush();
	return pieces.length > 0 ? pieces : [block.content];
}

/**
 * Build the section list for an oversized note.
 *
 * Sections are chunk-sized and carry the heading they came from, so a
 * long chapter costs a few seconds per section to prime rather than tens.
 * Returns an empty array when the body has no usable headings.
 */
export function markdownSections(body: string): MappedSection[] {
	const blocks = parseMarkdownBlocks(body);
	if (blocks.length === 0) return [];

	const sections: MappedSection[] = [];
	const seen = new Map<string, number>();

	for (const block of blocks) {
		const base = slug(block.label);
		const total = Math.max(block.content.length, 1);
		let consumed = 0;
		for (const piece of chunkBlock(block)) {
			const occurrence = seen.get(base) ?? 0;
			seen.set(base, occurrence + 1);
			const suffix = occurrence === 0 ? '' : `-${occurrence + 1}`;
			sections.push({
				key: `md:${base}${suffix}`,
				label: block.label.slice(0, MAX_LABEL),
				content: piece,
				headingIndex: block.headingIndex,
				startFraction: consumed / total,
				endFraction: Math.min(1, (consumed + piece.length) / total)
			});
			consumed += piece.length;
		}
	}

	return sections;
}

/**
 * Whether the editor's rendered headings line up with the parsed blocks.
 *
 * The active-section lookup is index-based, so a mismatch would attach one
 * section's text to another section's cached slot. Callers must skip section
 * emission entirely rather than act on a shifted mapping.
 */
export function canMapSections(blockCount: number, renderedHeadingCount: number): boolean {
	return blockCount > 0 && blockCount === renderedHeadingCount;
}

/**
 * Scroll offsets of the editor's rendered headings, in document order.
 *
 * This is the only DOM-aware step in this module; callers pair it with
 * `canMapSections` before trusting the positions.
 */
export function headingOffsets(container: HTMLElement | undefined): number[] {
	if (!container) return [];
	return Array.from(container.querySelectorAll<HTMLElement>('h1, h2, h3, h4, h5, h6')).map(
		(heading) => heading.offsetTop
	);
}

/**
 * Index of the heading in view: the last one that has scrolled to or past the
 * top of the viewport, so a heading flush against the top still counts.
 */
export function activeHeadingIndex(tops: number[], scrollTop: number): number {
	if (tops.length === 0) return -1;
	let active = 0;
	for (let i = 0; i < tops.length; i++) {
		if (tops[i] - 2 <= scrollTop) active = i;
		else break;
	}
	return active;
}

/**
 * Pick the section matching the viewport.
 *
 * First narrows to the heading in view, then — because one heading may own
 * several chunks — picks the chunk whose span in that heading's rendered range
 * is nearest the scroll position. `documentHeight` closes the range for the
 * last heading, which has no successor to bound it.
 */
export function activeSectionFor(
	sections: MappedSection[],
	tops: number[],
	scrollTop: number,
	documentHeight = Infinity
): MappedSection | null {
	const headingIndex = activeHeadingIndex(tops, scrollTop);
	if (headingIndex < 0) return null;
	const owned = sections.filter((section) => section.headingIndex === headingIndex);
	if (owned.length === 0) return null;
	if (owned.length === 1) return owned[0];

	const rangeTop = tops[headingIndex];
	const rangeBottom = headingIndex + 1 < tops.length ? tops[headingIndex + 1] : documentHeight;
	const span = rangeBottom - rangeTop;
	const offset = span > 0 ? (scrollTop - rangeTop) / span : 0;

	let best = owned[0];
	let bestDistance = Infinity;
	for (const section of owned) {
		const midpoint = (section.startFraction + section.endFraction) / 2;
		const distance = Math.abs(midpoint - offset);
		if (distance < bestDistance) {
			bestDistance = distance;
			best = section;
		}
	}
	return best;
}

/**
 * Strip the mapping metadata before sending a section to the backend, which
 * expects exactly `{ key, label, content }`.
 */
export function toBackendSection(section: MappedSection): PdfSection {
	return { key: section.key, label: section.label, content: section.content };
}
