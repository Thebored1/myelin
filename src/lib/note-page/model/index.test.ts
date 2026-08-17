import { describe, expect, it } from 'vitest';
import { reduceChatEvent } from './chatReducer';
import { documentType, sourceMaterialType } from './document';
import { parseBlocks, transclusionTarget } from './links';
import { cursorTarget, nearestIndexOf, selectionTarget } from './selection';

describe('note document model', () => {
	it('detects editable and source document types', () => {
		expect(documentType('a.TEX')).toBe('tex');
		expect(documentType('a.ipynb')).toBe('ipynb');
		expect(documentType('a.md')).toBe('md');
		expect(sourceMaterialType('book.EPUB')).toBe('epub');
		expect(sourceMaterialType('scan.PDF')).toBe('pdf');
		expect(sourceMaterialType('page.html')).toBe('html');
	});

	it('keeps block identifiers and resolves transclusions', () => {
		expect(parseBlocks('## [Alpha](https://example.test) ((a1b2c3))\nBeta', 'n1', 'Note')).toEqual([
			{
				id: 'a1b2c3',
				original: '## [Alpha](https://example.test) ((a1b2c3))',
				text: 'Alpha',
				sourceNoteId: 'n1',
				sourceNoteTitle: 'Note'
			},
			{
				id: null,
				original: 'Beta',
				text: 'Beta',
				sourceNoteId: 'n1',
				sourceNoteTitle: 'Note'
			}
		]);
		expect(transclusionTarget('/notes/hello%20world#a1b2c3')).toEqual({
			noteId: 'hello world',
			blockId: 'a1b2c3'
		});
	});

	it('matches selections by anchor proximity', () => {
		expect(nearestIndexOf('x target y target', 'target', 10)).toBe(11);
		expect(selectionTarget('0123456789', 3, 6)).toMatchObject({
			text: '345',
			before: '012',
			after: '6789'
		});
		expect(cursorTarget('abcdef', 3)).toMatchObject({ cursor: true, before: 'abc', after: 'def' });
	});

	it('reduces only events for the active request', () => {
		const initial = [{ role: 'assistant', content: 'old', isStreaming: true }];
		expect(
			reduceChatEvent(initial, { type: 'chunk', requestId: 'other', delta: '!' }, 'active')
		).toBe(initial);
		expect(
			reduceChatEvent(initial, { type: 'chunk', requestId: 'active', delta: '!' }, 'active')[0]
				.content
		).toBe('old!');
	});
});
