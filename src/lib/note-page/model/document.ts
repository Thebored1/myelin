export type NoteDocumentType = 'md' | 'tex' | 'ipynb';
export type SourceMaterialType = 'pdf' | 'epub' | 'html' | null;

export function documentType(relativePath: string): NoteDocumentType {
	const path = relativePath.toLowerCase();
	if (path.endsWith('.tex')) return 'tex';
	if (path.endsWith('.ipynb')) return 'ipynb';
	return 'md';
}

export function sourceMaterialType(relativePath: string): SourceMaterialType {
	const path = relativePath.toLowerCase();
	if (path.endsWith('.pdf')) return 'pdf';
	if (path.endsWith('.epub')) return 'epub';
	if (path.endsWith('.html') || path.endsWith('.htm')) return 'html';
	return null;
}

export function isBinarySource(relativePath: string): boolean {
	return sourceMaterialType(relativePath) !== null;
}

