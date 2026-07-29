import { describe, expect, it } from 'vitest';
import {
	composeNoteStreamPreview,
	composeNoteStreamPreviewWithStatus,
	locateNoteStreamTarget
} from './noteStreamPreview';

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

	it.each(['\n', '   ', '\t', '\n\n\t  '])(
		'streams an unanchored cursor write over a visually empty note (%j)',
		(source) => {
			const target = { text: '', before: '', after: '', cursor: true };
			expect(locateNoteStreamTarget(source, target)).toEqual([0, source.length]);
			expect(composeNoteStreamPreview(source, 'Roses', target)).toBe('Roses');
			expect(composeNoteStreamPreview(source, 'Roses are red', target)).toBe('Roses are red');
		}
	);

	it('makes every progressive delta match the growing backend body without blank residue', () => {
		const target = { text: '', before: '', after: '', cursor: true };
		const deltas = ['Roses', ' are red', '\nViolets', ' are blue'];
		let generated = '';
		for (const delta of deltas) {
			generated += delta;
			expect(composeNoteStreamPreview('\n', generated, target)).toBe(generated);
		}
	});

	it('does not modify the preview when cursor anchors are ambiguous', () => {
		const source = 'same same';
		const target = { text: '', before: 'same', after: '', cursor: true };
		expect(locateNoteStreamTarget(source, target)).toBeNull();
		expect(composeNoteStreamPreview(source, 'new', target)).toBe(source);
	});

	it('never treats empty cursor anchors as an insertion point in non-empty content', () => {
		const source = 'existing note';
		const target = { text: '', before: '', after: '', cursor: true };
		expect(locateNoteStreamTarget(source, target)).toBeNull();
		expect(composeNoteStreamPreviewWithStatus(source, 'new', target)).toEqual({
			preview: source,
			applied: false
		});
	});

	it('replaces ordinary selected text while streaming', () => {
		const source = 'before old after';
		const target = { text: 'old', before: 'before ', after: ' after', cursor: false };
		expect(composeNoteStreamPreview(source, 'new', target)).toBe('before new after');
	});

	it('uses generated content as the whole-note preview without an editor target', () => {
		expect(composeNoteStreamPreview('old', 'new', null)).toBe('new');
		expect(composeNoteStreamPreviewWithStatus('old', 'new', null).applied).toBe(true);
	});
});
