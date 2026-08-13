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
		if (!toolbar) {
			ctx.toolbarNeedsToggle = false;
			return;
		}
		const items = toolbar.querySelectorAll('.vditor-toolbar__item, .vditor-toolbar__divider');
		if (items.length === 0) {
			ctx.toolbarNeedsToggle = false;
			return;
		}

		// Measure the complete unhidden row, then let CSS clip overflow in
		// collapsed mode and wrap only when the user expands the toolbar.
		const toolbarStyle = getComputedStyle(toolbar);
		const paddingLeft = parseFloat(toolbarStyle.paddingLeft) || 0;
		const paddingRight = parseFloat(toolbarStyle.paddingRight) || 0;
		const availableWidth = toolbar.clientWidth - paddingLeft - paddingRight;
		const itemElements = Array.from(items) as HTMLElement[];
		const firstRowTop = Math.min(...itemElements.map((item) => item.getBoundingClientRect().top));
		const hasWrappedRow = itemElements.some(
			(item) => item.getBoundingClientRect().top > firstRowTop + 2
		);
		const totalWidth = itemElements.reduce<number>((width, item) => {
			const element = item as HTMLElement;
			const style = getComputedStyle(element);
			return (
				width +
				 element.getBoundingClientRect().width +
				 (parseFloat(style.marginLeft) || 0) +
				 (parseFloat(style.marginRight) || 0)
			);
		}, 0);

		const needsToggle =
			hasWrappedRow || (availableWidth > 0 && totalWidth > availableWidth + 1);
		ctx.toolbarNeedsToggle = needsToggle;
		if (!needsToggle) ctx.toolbarExpanded = false;

		// Clear styles left by older toolbar instances or a prior measurement.
		items.forEach((item: Element) => {
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
		ctx.toolbarExpanded;
		updateToolbarOverflow();
	});

	return { openNoteNotebook, localVditorCdn, activeAiNoteId, updateToolbarOverflow };
}
