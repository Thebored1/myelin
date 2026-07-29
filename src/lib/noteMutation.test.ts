import { describe, expect, it } from 'vitest';
import {
	canApplyReconciledNote,
	editorNeedsAuthoritativeBody,
	hasNoteMutation
} from './noteMutation';

describe('note mutation completion reconciliation', () => {
	it.each([
		'Write Note',
		'Replace Text',
		'Append Note',
		'Prepend Note',
		'Insert After',
		'Delete Text',
		'Clear Note',
		'Format Note',
		'Add Cell',
		'Delete Cell',
		'Edit Cell'
	])('classifies %s as a note mutation', (name) => {
		expect(hasNoteMutation([{ name, details: 'any target description' }])).toBe(true);
	});

	it('does not reconcile read-only tools', () => {
		expect(hasNoteMutation([{ name: 'Search Notes', details: 'poems' }])).toBe(false);
	});

	it('applies a loaded document only while the request target remains open', () => {
		expect(canApplyReconciledNote('note-a', 'note-a')).toBe(true);
		expect(canApplyReconciledNote('note-a', 'note-b')).toBe(false);
		expect(canApplyReconciledNote('note-a', null)).toBe(false);
	});

	it('does not visibly reset an editor that already has the saved body', () => {
		expect(editorNeedsAuthoritativeBody('final body', 'final body')).toBe(false);
		expect(editorNeedsAuthoritativeBody('stale preview', 'final body')).toBe(true);
	});
});
