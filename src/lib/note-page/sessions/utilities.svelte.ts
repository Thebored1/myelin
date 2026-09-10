import { base } from '$app/paths';
import { resolveActiveAiTarget } from '$lib/aiTarget';
import type { ControllerContext } from '$lib/controller-context';

/** Small browser-facing helpers shared by the note page composition root. */
export function createNotePageUtilities(rawContext: object) {
	const ctx = rawContext as ControllerContext;
	function openNoteNotebook(): string | null {
		const currentNote = ctx.note;
		if (!currentNote) return null;
		const segments = currentNote.relativePath.replace(/\\/g, '/').split('/').filter(Boolean);
		return segments.length > 1 ? segments[0] : null;
	}

	function localVditorCdn() {
		const appPath = `${base}/vditor/`.replace(/\/+/g, '/');
		// URL construction is a one-shot browser helper; the URL is not mutated.
		// eslint-disable-next-line svelte/prefer-svelte-reactivity
		return new URL(
			appPath.startsWith('/') ? appPath : `/${appPath}`,
			document.baseURI
		).href.replace(/\/$/, '');
	}

	function activeAiNoteId(): string | null {
		return (
			resolveActiveAiTarget({
				openedDocumentId: ctx.note?.id ?? null,
				isSourceMaterial: ctx.isSourceMaterial,
				attachedNoteVisible: ctx.showAttachedNote,
				workingNoteId: ctx.scratchpadSavedId,
				attachedSourceId: ctx.activeSourceId
			})?.workingNoteId ?? null
		);
	}

	function updateToolbarOverflow() {
		const toolbar = ctx.vditorContainer?.querySelector('.vditor-toolbar');
		// The toolbar is intentionally always visible and wrapping. Keep the old
		// state fields neutral for callers that still pass them through the graph,
		// but do not measure or hide items behind a custom overflow toggle.
		ctx.toolbarNeedsToggle = false;
		ctx.toolbarExpanded = false;
		if (!toolbar) return;
		toolbar
			.querySelectorAll('.vditor-toolbar__item, .vditor-toolbar__divider')
			.forEach((item: Element) => {
				const element = item as HTMLElement;
				element.style.removeProperty('visibility');
				element.style.removeProperty('pointer-events');
			});
	}

	$effect(() => {
		if (ctx.sourceMaterialType === 'pdf' && !ctx.PdfViewerComponent) {
			import('$lib/components/PdfViewer.svelte').then(({ default: component }) => {
				ctx.PdfViewerComponent = component;
			});
		}
		if (ctx.sourceMaterialType === 'epub' && !ctx.EpubViewerComponent) {
			import('$lib/components/EpubViewer.svelte').then(({ default: component }) => {
				ctx.EpubViewerComponent = component;
			});
		}
		if (ctx.sourceMaterialType === 'html' && !ctx.HtmlViewerComponent) {
			import('$lib/components/HtmlViewer.svelte').then(({ default: component }) => {
				ctx.HtmlViewerComponent = component;
			});
		}
		if (ctx.workingDocType === 'tex' && !ctx.TexEditorComponent) {
			import('$lib/components/TexEditor.svelte').then(({ default: component }) => {
				ctx.TexEditorComponent = component;
			});
		}
		if (ctx.workingDocType === 'ipynb' && !ctx.IpynbEditorComponent) {
			import('$lib/components/IpynbEditor.svelte').then(({ default: component }) => {
				ctx.IpynbEditorComponent = component;
			});
		}
	});

	$effect(() => {
		void ctx.toolbarExpanded;
		updateToolbarOverflow();
	});

	return { openNoteNotebook, localVditorCdn, activeAiNoteId, updateToolbarOverflow };
}
