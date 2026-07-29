const NOTE_MUTATION_TOOLS = new Set([
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
]);

export type ChatTool = { name: string; details: string };

export function hasNoteMutation(tools: ChatTool[] = []): boolean {
	return tools.some((tool) => NOTE_MUTATION_TOOLS.has(tool.name));
}

export function canApplyReconciledNote(
	expectedNoteId: string,
	currentNoteId: string | null | undefined
): boolean {
	return currentNoteId === expectedNoteId;
}

export function editorNeedsAuthoritativeBody(currentBody: string, savedBody: string): boolean {
	return currentBody !== savedBody;
}
