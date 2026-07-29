import { describe, expect, it } from 'vitest';
import { composeNoteStreamPreview, locateNoteStreamTarget } from './noteStreamPreview';

describe('note stream previews', () => {
	it('replaces only the anchored occurrence of repeated selected text', () => {
		const source = 'alpha repeat middle repeat omega';
		const target = {
			text: 'repeat',
			before: 'alpha ',
			after: ' middle',
			cursor: false
		};
		expect(composeNoteStreamPreview(source, 'growing', target)).toBe(
			'alpha growing middle repeat omega'
		);
	});

	it('preserves Markdown outside a selected source span', () => {
		const source = '# Title\nA **bold phrase** remains.\n';
		const target = {
			text: '**bold phrase**',
			before: '# Title\nA ',
			after: ' remains.\n',
			cursor: false
		};
		expect(composeNoteStreamPreview(source, '*new text*', target)).toBe(
			'# Title\nA *new text* remains.\n'
		);
	});

	it('inserts a growing cursor write at its unique anchors', () => {
		const source = 'before\nafter';
		const target = { text: '', before: 'before\n', after: 'after', cursor: true };
		expect(composeNoteStreamPreview(source, 'new\n', target)).toBe('before\nnew\nafter');
	});

	it('does not modify the preview when cursor anchors are ambiguous', () => {
		const source = 'same same';
		const target = { text: '', before: 'same', after: '', cursor: true };
		expect(locateNoteStreamTarget(source, target)).toBeNull();
		expect(composeNoteStreamPreview(source, 'new', target)).toBe(source);
	});

	it('uses generated content as the whole-note preview without an editor target', () => {
		expect(composeNoteStreamPreview('old', 'new', null)).toBe('new');
	});
});
