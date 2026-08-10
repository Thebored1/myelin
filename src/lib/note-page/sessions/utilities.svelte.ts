import { base } from '$app/paths';
import { resolveActiveAiTarget } from '$lib/aiTarget';

/** Small browser-facing helpers shared by the note page composition root. */
export function createNotePageUtilities(ctx: Record<string, any>) {
	function openNoteNotebook(): string | null {
		const currentNote = ctx.note;
		if (!currentNote) return null;
		const segments = currentNote.relativePath.replace(/\\/g, '/').split('/').filter(Boolean);
		return segments.length > 1 ? segments[0] : null;
	}

	function localVditorCdn() {
		const appPath = `${base}/vditor/`.replace(/\/+/g, '/');
		return new URL(appPath.startsWith('/') ? appPath : `/${appPath}`, document.baseURI).href.replace(
			/\/$/,
			''
		);
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
		if (!toolbar) return;
		const items = toolbar.querySelectorAll('.vditor-toolbar__item, .vditor-toolbar__divider');
		items.forEach((item: any) => {
			if (!ctx.toolbarExpanded && item.offsetTop > 20) {
				item.style.visibility = 'hidden';
				item.style.pointerEvents = 'none';
			} else {
				item.style.visibility = 'visible';
				item.style.pointerEvents = 'auto';
			}
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
		ctx.toolbarExpanded;
		updateToolbarOverflow();
	});

	return { openNoteNotebook, localVditorCdn, activeAiNoteId, updateToolbarOverflow };
}
