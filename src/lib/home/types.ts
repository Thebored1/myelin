import type { NoteSummary } from '$lib/types';

export type NoteType = 'md' | 'pdf' | 'tex' | 'ipynb' | 'epub';
export type NoteFilter = 'all' | 'notes' | 'documents' | NoteType;
export type NotebookFilter = string | null;
export type HomeNote = NoteSummary;
