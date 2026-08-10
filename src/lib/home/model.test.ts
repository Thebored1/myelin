import { describe, expect, it } from 'vitest';
import { filterNotes, isAttachmentCopy, noteType, timeAgo } from './model';
import type { NoteSummary } from '$lib/types';

const note = (id: string, path: string, folder = ''): NoteSummary => ({
	id, title: id, tags: ['work'], folder, excerpt: '', relativePath: path,
	createdAt: '2026-01-01T00:00:00Z', updatedAt: '2026-01-01T00:00:00Z', backlinks: []
});

describe('home model', () => {
	it('detects note types and attachment copies', () => {
		expect(noteType(note('a', 'A.TEX'))).toBe('tex');
		expect(isAttachmentCopy(note('copy', 'source 2.pdf'), new Set(['copy']))).toBe(true);
		expect(isAttachmentCopy(note('original', 'source.pdf'), new Set(['original']))).toBe(false);
	});

	it('filters notebook notes and puts pinned notes first', () => {
		const notes = [note('a', 'a.md', 'Research'), note('b', 'b.md', 'Research/Ideas'), note('c', 'c.pdf')];
		expect(filterNotes(notes, 'notes', 'work', 'Research', new Set(), ['b']).map((n) => n.id)).toEqual(['b', 'a']);
	});

	it('formats relative age deterministically', () => {
		const now = Date.parse('2026-01-08T00:00:00Z');
		expect(timeAgo('2026-01-08T00:00:00Z', now)).toBe('now');
		expect(timeAgo('2026-01-07T23:00:00Z', now)).toBe('1h');
		expect(timeAgo('2025-12-01T00:00:00Z', now)).toBe('5w');
	});
});
