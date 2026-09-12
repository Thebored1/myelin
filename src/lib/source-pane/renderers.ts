import { renderAsync } from 'docx-preview';
import DOMPurify from 'dompurify';
import { rtfToHtml } from './rtf';
import type { SourceKind } from './types';

export const SOURCE_EXTENSIONS: Record<SourceKind, string[]> = {
	html: ['html', 'htm'],
	txt: ['txt', 'md'],
	docx: ['docx'],
	rtf: ['rtf']
};

export function kindForPath(path: string): SourceKind | null {
	const lower = path.toLowerCase();
	for (const [kind, extensions] of Object.entries(SOURCE_EXTENSIONS)) {
		if (extensions.some((extension) => lower.endsWith(`.${extension}`))) {
			return kind as SourceKind;
		}
	}
	return null;
}

function decode(bytes: Uint8Array): string {
	return new TextDecoder('utf-8').decode(bytes);
}

function sanitize(html: string): string {
	return DOMPurify.sanitize(html, {
		FORBID_TAGS: ['script', 'style', 'iframe', 'object', 'embed', 'link', 'meta'],
		FORBID_ATTR: ['onerror', 'onload', 'onclick']
	});
}

/**
 * Renders a text-bearing source document into the pane container. Every
 * format resolves to the same thing — styled, text-bearing DOM — which is
 * what lets the shared annotation tools work uniformly.
 */
export async function renderSource(
	kind: SourceKind,
	bytes: Uint8Array,
	container: HTMLElement
): Promise<{ getText: () => string }> {
	switch (kind) {
		case 'txt': {
			const pre = document.createElement('pre');
			pre.className = 'source-plain';
			pre.textContent = decode(bytes);
			container.replaceChildren(pre);
			break;
		}
		case 'html': {
			container.innerHTML = sanitize(decode(bytes));
			break;
		}
		case 'docx': {
			// Layout-faithful rendering (alignment, fonts, sizes, page breaks) —
			// a semantic-HTML converter would flatten centering and type sizes.
			container.classList.add('docx-host');
			const copy = new ArrayBuffer(bytes.byteLength);
			new Uint8Array(copy).set(bytes);
			await renderAsync(copy, container, undefined, {
				inWrapper: true,
				ignoreLastRenderedPageBreak: false,
				experimental: true,
				useBase64URL: true
			});
			break;
		}
		case 'rtf': {
			container.innerHTML = sanitize(rtfToHtml(decode(bytes)));
			break;
		}
	}
	return { getText: () => container.innerText ?? '' };
}
