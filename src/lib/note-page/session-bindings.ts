import type { createLinkingSession } from './sessions/linking.svelte';
import type { createMathSession } from './sessions/math.svelte';

type LinkingSession = ReturnType<typeof createLinkingSession>;
type MathSession = ReturnType<typeof createMathSession>;

/** Keeps dialog/component bindings connected to the stateful sessions that own them. */
export function createNotePageSessionBindings(
	mathSession: MathSession,
	linkingSession: LinkingSession
) {
	return {
		read: () => ({
			mathDialog: mathSession.mathDialog,
			mathValue: mathSession.mathValue,
			mathLiveReady: mathSession.mathLiveReady,
			katexRenderer: mathSession.katexRenderer,
			mathError: mathSession.mathError,
			linkNoteDialog: linkingSession.linkNoteDialog,
			linkSearchQuery: linkingSession.linkSearchQuery,
			linkSearchResults: linkingSession.linkSearchResults,
			linkSelectedIndex: linkingSession.linkSelectedIndex,
			linkDialogMode: linkingSession.linkDialogMode,
			selectedNoteForBlocks: linkingSession.selectedNoteForBlocks,
			allNoteBlocks: linkingSession.allNoteBlocks,
			filteredBlocks: linkingSession.filteredBlocks,
			previewNoteDialog: linkingSession.previewNoteDialog,
			previewNoteTarget: linkingSession.previewNoteTarget,
			previewNoteContainer: linkingSession.previewNoteContainer,
			globalSearchDialog: linkingSession.globalSearchDialog,
			globalSearchQuery: linkingSession.globalSearchQuery,
			globalSelectedIndex: linkingSession.globalSelectedIndex,
			globalBlocks: linkingSession.globalBlocks,
			filteredGlobalBlocks: linkingSession.filteredGlobalBlocks
		}),
		write: {
			mathDialog: (value: MathSession['mathDialog']) => (mathSession.mathDialog = value),
			mathValue: (value: string) => (mathSession.mathValue = value),
			mathLiveReady: (value: boolean) => (mathSession.mathLiveReady = value),
			katexRenderer: (value: MathSession['katexRenderer']) => (mathSession.katexRenderer = value),
			mathError: (value: string) => (mathSession.mathError = value),
			linkNoteDialog: (value: LinkingSession['linkNoteDialog']) =>
				(linkingSession.linkNoteDialog = value),
			linkSearchQuery: (value: string) => (linkingSession.linkSearchQuery = value),
			linkSearchResults: (value: LinkingSession['linkSearchResults']) =>
				(linkingSession.linkSearchResults = value),
			linkSelectedIndex: (value: number) => (linkingSession.linkSelectedIndex = value),
			linkDialogMode: (value: LinkingSession['linkDialogMode']) =>
				(linkingSession.linkDialogMode = value),
			selectedNoteForBlocks: (value: LinkingSession['selectedNoteForBlocks']) =>
				(linkingSession.selectedNoteForBlocks = value),
			allNoteBlocks: (value: LinkingSession['allNoteBlocks']) =>
				(linkingSession.allNoteBlocks = value),
			previewNoteDialog: (value: LinkingSession['previewNoteDialog']) =>
				(linkingSession.previewNoteDialog = value),
			previewNoteTarget: (value: LinkingSession['previewNoteTarget']) =>
				(linkingSession.previewNoteTarget = value),
			previewNoteContainer: (value: LinkingSession['previewNoteContainer']) =>
				(linkingSession.previewNoteContainer = value),
			globalSearchDialog: (value: LinkingSession['globalSearchDialog']) =>
				(linkingSession.globalSearchDialog = value),
			globalSearchQuery: (value: string) => (linkingSession.globalSearchQuery = value),
			globalSelectedIndex: (value: number) => (linkingSession.globalSelectedIndex = value),
			globalBlocks: (value: LinkingSession['globalBlocks']) => (linkingSession.globalBlocks = value)
		},
		mathActions: {
			mathToKatex: mathSession.mathToKatex,
			openMathDialog: mathSession.openMathDialog,
			insertMath: mathSession.insertMath
		}
	};
}
