import { describe, expect, it } from 'vitest';
import { resolveActiveAiTarget } from './aiTarget';

describe('active AI target resolution', () => {
	it('uses the working note for the note + PDF dashboard path', () => {
		expect(
			resolveActiveAiTarget({
				openedDocumentId: 'note-1',
				isSourceMaterial: false,
				attachedNoteVisible: true,
				workingNoteId: 'note-1',
				attachedSourceId: 'pdf-1'
			})
		).toEqual({
			workingNoteId: 'note-1',
			allowedDocumentIds: ['note-1', 'pdf-1'],
			readOnly: false
		});
	});

	it('resolves the PDF attached-note pane to the same working note identity', () => {
		expect(
			resolveActiveAiTarget({
				openedDocumentId: 'pdf-1',
				isSourceMaterial: true,
				attachedNoteVisible: true,
				workingNoteId: 'note-1',
				attachedSourceId: 'pdf-1'
			})
		).toMatchObject({ workingNoteId: 'note-1', allowedDocumentIds: ['note-1', 'pdf-1'] });
	});

	it('keeps a PDF-only view read-only and scoped to that PDF', () => {
		expect(
			resolveActiveAiTarget({
				openedDocumentId: 'pdf-1',
				isSourceMaterial: true,
				attachedNoteVisible: false,
				workingNoteId: null,
				attachedSourceId: 'pdf-1'
			})
		).toEqual({ workingNoteId: 'pdf-1', allowedDocumentIds: ['pdf-1'], readOnly: true });
	});
});
