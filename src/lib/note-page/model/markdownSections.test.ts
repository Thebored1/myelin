import { describe, expect, it } from 'vitest';
import {
	activeHeadingIndex,
	activeSectionFor,
	canMapSections,
	oversizedCharLimit,
	markdownSections,
	parseMarkdownBlocks,
	shouldSectionNote,
	toBackendSection,
	type MappedSection
} from './markdownSections';

const section = (
	over: Partial<MappedSection> & Pick<MappedSection, 'key' | 'content'>
): MappedSection => ({
	label: 'L',
	headingIndex: 0,
	startFraction: 0,
	endFraction: 1,
	...over
});

describe('markdown sectioning', () => {
	describe('oversize gate', () => {
		it('mirrors the backend clamp', () => {
			// NotePromptShape::build: ctx * 2 chars, clamped to [4000, 400000].
			expect(oversizedCharLimit(32768)).toBe(65536);
			expect(oversizedCharLimit(1024)).toBe(4000);
			expect(oversizedCharLimit(400_000)).toBe(400_000);
		});

		it('leaves a note that fits in context unsectioned', () => {
			expect(shouldSectionNote(2_000, 32768)).toBe(false);
			expect(shouldSectionNote(65_536, 32768)).toBe(false);
			expect(shouldSectionNote(65_537, 32768)).toBe(true);
		});
	});

	describe('block parsing', () => {
		it('splits on ATX headings and folds the preamble into the first', () => {
			const blocks = parseMarkdownBlocks('leading prose\n\n# Intro\n\na\n\n## Details\n\nb\n');
			expect(blocks).toHaveLength(2);
			expect(blocks[0].label).toBe('Intro');
			expect(blocks[0].content).toContain('leading prose');
			expect(blocks[1].headingIndex).toBe(1);
		});

		it('ignores headings inside fenced code blocks', () => {
			const body = ['# Real', '```bash', '# fake', '## also fake', '```', '## Second'].join('\n');
			expect(parseMarkdownBlocks(body).map((b) => b.label)).toEqual(['Real', 'Second']);
		});

		it('returns nothing for a body with no headings', () => {
			expect(parseMarkdownBlocks('plain prose only')).toEqual([]);
			expect(parseMarkdownBlocks('   ')).toEqual([]);
		});
	});

	describe('chunking', () => {
		it('keeps a short heading as one section', () => {
			expect(markdownSections('# A\n\nshort body\n')).toHaveLength(1);
		});

		it('splits a long heading into several sections that keep the label', () => {
			const long = 'x'.repeat(6000);
			const sections = markdownSections(`# Long\n\n${long}\n`);
			expect(sections.length).toBeGreaterThan(1);
			expect(sections.every((s) => s.label === 'Long')).toBe(true);
			// Each piece stays within the hard maximum.
			for (const piece of sections) expect(piece.content.length).toBeLessThanOrEqual(1536);
		});

		it('keeps chunk keys unique within one heading', () => {
			const sections = markdownSections(`# Long\n\n${'y'.repeat(9000)}\n`);
			const keys = sections.map((s) => s.key);
			expect(new Set(keys).size).toBe(keys.length);
		});

		it('splits on paragraph boundaries rather than mid-word', () => {
			const para = 'alpha beta gamma delta. '.repeat(40);
			const sections = markdownSections(`# P\n\n${para}\n\n${para}\n`);
			expect(sections.length).toBeGreaterThan(1);
		});
	});

	describe('active section', () => {
		it('treats a heading flush with the viewport top as active', () => {
			expect(activeHeadingIndex([0, 200, 400], 0)).toBe(0);
			expect(activeHeadingIndex([0, 200, 400], 150)).toBe(0);
			expect(activeHeadingIndex([0, 200, 400], 205)).toBe(1);
			expect(activeHeadingIndex([], 10)).toBe(-1);
		});

		it('picks the chunk nearest the scroll position within one heading', () => {
			const sections = [
				section({ key: 'a', content: 'a', startFraction: 0, endFraction: 0.33 }),
				section({ key: 'b', content: 'b', startFraction: 0.33, endFraction: 0.66 }),
				section({ key: 'c', content: 'c', startFraction: 0.66, endFraction: 1 })
			];
			expect(activeSectionFor(sections, [0], 0, 1000)?.key).toBe('a');
			expect(activeSectionFor(sections, [0], 500, 1000)?.key).toBe('b');
			expect(activeSectionFor(sections, [0], 950, 1000)?.key).toBe('c');
		});

		it('narrows to the heading in view before choosing a chunk', () => {
			const sections = [
				section({ key: 'h0a', content: 'a', headingIndex: 0 }),
				section({ key: 'h1a', content: 'b', headingIndex: 1 })
			];
			expect(activeSectionFor(sections, [0, 300], 0, 1000)?.key).toBe('h0a');
			expect(activeSectionFor(sections, [0, 300], 350, 1000)?.key).toBe('h1a');
		});

		it('refuses to map when rendered headings disagree with blocks', () => {
			expect(canMapSections(2, 1)).toBe(false);
			expect(canMapSections(0, 0)).toBe(false);
			expect(canMapSections(2, 2)).toBe(true);
		});
	});

	it('sends only key/label/content to the backend', () => {
		const mapped = section({
			key: 'md:intro',
			content: 'body',
			label: 'Intro',
			headingIndex: 0,
			startFraction: 0.1,
			endFraction: 0.9
		});
		expect(toBackendSection(mapped)).toEqual({ key: 'md:intro', label: 'Intro', content: 'body' });
	});
});
