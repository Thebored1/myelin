const ESCAPES: Record<string, string> = {
	'~': '&nbsp;',
	'{': '{',
	'}': '}',
	'\\': '\\'
};

const SKIP_DESTINATIONS = new Set([
	'fonttbl',
	'colortbl',
	'stylesheet',
	'info',
	'pict',
	'object',
	'header',
	'footer',
	'themedata',
	'latentstyles',
	'datastore',
	'listtable',
	'listoverridetable',
	'rsidtbl',
	'generator',
	'*'
]);

type FormatState = {
	bold: number;
	italic: number;
	underline: number;
	strike: number;
	skip: boolean;
};

const INITIAL_STATE: FormatState = { bold: 0, italic: 0, underline: 0, strike: 0, skip: false };

function escapeHtml(text: string): string {
	return text.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
}

function openTags(state: FormatState): string {
	let tags = '';
	if (state.bold > 0) tags += '<b>';
	if (state.italic > 0) tags += '<i>';
	if (state.underline > 0) tags += '<u>';
	if (state.strike > 0) tags += '<s>';
	return tags;
}

function closeTags(state: FormatState): string {
	let tags = '';
	if (state.strike > 0) tags += '</s>';
	if (state.underline > 0) tags += '</u>';
	if (state.italic > 0) tags += '</i>';
	if (state.bold > 0) tags += '</b>';
	return tags;
}

/**
 * Converts RTF to plain, sanitized-friendly HTML. Handles the subset of RTF
 * word processors emit for prose: groups with scoped character formatting,
 * breaks, hex and \\uN unicode escapes, and skips non-prose destinations
 * (font/color tables, pictures, headers). A viewing converter, not a
 * layout-faithful RTF engine.
 */
export function rtfToHtml(rtf: string): string {
	let state: FormatState = { ...INITIAL_STATE };
	const stack: FormatState[] = [];
	let out = '';
	let paragraph: string[] = [];
	let emitted: FormatState | null = null;
	let index = 0;

	// Emit text with formatting tags only when the format state changes, so
	// words come out as <b>bold</b> instead of <b>b</b><b>o</b><b>l</b><b>d</b>.
	const emit = (text: string) => {
		if (emitted === null) {
			paragraph.push(openTags(state) + text);
		} else if (
			emitted.bold !== state.bold ||
			emitted.italic !== state.italic ||
			emitted.underline !== state.underline ||
			emitted.strike !== state.strike
		) {
			paragraph.push(closeTags(emitted) + openTags(state) + text);
		} else {
			paragraph.push(text);
		}
		emitted = { ...state };
	};

	const flushParagraph = () => {
		if (emitted !== null) {
			paragraph.push(closeTags(emitted));
			emitted = null;
		}
		const content = paragraph.join('');
		if (content.trim().length > 0) out += `<p>${content}</p>`;
		paragraph = [];
	};

	while (index < rtf.length) {
		const char = rtf[index];
		if (char === '{') {
			stack.push({ ...state });
			index += 1;
			continue;
		}
		if (char === '}') {
			state = stack.pop() ?? { ...INITIAL_STATE };
			index += 1;
			continue;
		}
		if (char === '\\' && state.skip) {
			// Control words inside skipped destinations still consume tokens.
			const match = /^\\([a-zA-Z]+)(-?\d+)? ?/.exec(rtf.slice(index));
			if (match) {
				index += match[0].length;
				continue;
			}
			const hex = /^\\'([0-9a-fA-F]{2})/.exec(rtf.slice(index));
			if (hex) {
				index += 4;
				continue;
			}
			index += 2;
			continue;
		}
		if (char === '\\') {
			const match = /^\\([a-zA-Z]+)(-?\d+)? ?/.exec(rtf.slice(index));
			if (match) {
				const word = match[1];
				index += match[0].length;
				switch (word) {
					case 'b':
						state.bold += 1;
						break;
					case 'i':
						state.italic += 1;
						break;
					case 'ul':
						state.underline += 1;
						break;
					case 'strike':
						state.strike += 1;
						break;
					case 'plain':
						state.bold = 0;
						state.italic = 0;
						state.underline = 0;
						state.strike = 0;
						break;
					case 'par':
					case 'sect':
						flushParagraph();
						break;
					case 'line':
						emit('<br>');
						break;
					case 'tab':
						paragraph.push('    ');
						break;
					case 'u': {
						const code = parseInt(match[2] ?? '0', 10);
						const normalized = code < 0 ? code + 65536 : code;
						paragraph.push(escapeHtml(String.fromCharCode(normalized)));
						// \uN is followed by N fallback characters we must skip.
						state.skip = false;
						let pending = 1;
						while (pending > 0 && index < rtf.length) {
							if (rtf[index] === '\\') {
								const fallback = /^\\'([0-9a-fA-F]{2})|^\\[a-zA-Z]+-?\d* ?|^\\./.exec(
									rtf.slice(index)
								);
								index += fallback ? fallback[0].length : 2;
								pending -= 1;
								continue;
							}
							if (rtf[index] === '{' || rtf[index] === '}') {
								// Fallback runs are usually a braced group; skip into it.
								index += 1;
								continue;
							}
							index += 1;
							pending -= 1;
						}
						break;
					}
					default:
						if (SKIP_DESTINATIONS.has(word)) {
							// Hide the rest of this group: peek ahead for '{'.
							const rest = rtf.slice(index);
							if (rest.startsWith('{')) {
								state.skip = true;
							}
						}
						break;
				}
				continue;
			}
			const hex = /^\\'([0-9a-fA-F]{2})/.exec(rtf.slice(index));
			if (hex) {
				index += 4;
				emit(escapeHtml(String.fromCharCode(parseInt(hex[1], 16))));
				continue;
			}
			const escaped = rtf[index + 1];
			if (escaped && ESCAPES[escaped]) {
				paragraph.push(ESCAPES[escaped]);
				index += 2;
				continue;
			}
			if (escaped === '\n' || escaped === '\r') {
				paragraph.push(' ');
				index += 2;
				continue;
			}
			index += 2;
			continue;
		}
		if (char === '\r' || char === '\n') {
			index += 1;
			continue;
		}
		if (!state.skip) emit(escapeHtml(char));
		index += 1;
	}
	flushParagraph();
	return out || '<p></p>';
}

/** Extract plain text from RTF — used for search ingestion. */
export function rtfToText(rtf: string): string {
	return rtfToHtml(rtf)
		.replace(/<[^>]+>/g, ' ')
		.replace(/&nbsp;/g, ' ')
		.replace(/&amp;/g, '&')
		.replace(/&lt;/g, '<')
		.replace(/&gt;/g, '>')
		.replace(/\s+/g, ' ')
		.trim();
}
