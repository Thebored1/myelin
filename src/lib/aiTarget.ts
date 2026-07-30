export interface ActiveAiTargetInput {
	openedDocumentId: string | null;
	isSourceMaterial: boolean;
	attachedNoteVisible: boolean;
	workingNoteId: string | null;
	attachedSourceId: string | null;
}

export interface ActiveAiTarget {
	workingNoteId: string;
	allowedDocumentIds: string[];
	readOnly: boolean;
}

/** Resolve the one identity used by chat, history, cache warm-up, and tools. */
export function resolveActiveAiTarget(input: ActiveAiTargetInput): ActiveAiTarget | null {
	if (!input.openedDocumentId) return null;

	if (input.isSourceMaterial) {
		if (input.attachedNoteVisible && input.workingNoteId) {
			return {
				workingNoteId: input.workingNoteId,
				allowedDocumentIds: [input.workingNoteId, input.attachedSourceId ?? input.openedDocumentId],
				readOnly: false
			};
		}
		return {
			workingNoteId: input.openedDocumentId,
			allowedDocumentIds: [input.openedDocumentId],
			readOnly: true
		};
	}

	return {
		workingNoteId: input.openedDocumentId,
		allowedDocumentIds: input.attachedSourceId
			? [input.openedDocumentId, input.attachedSourceId]
			: [input.openedDocumentId],
		readOnly: false
	};
}
