const MODIFIER_CODES = new Set([
	'ControlLeft',
	'ControlRight',
	'AltLeft',
	'AltRight',
	'ShiftLeft',
	'ShiftRight',
	'MetaLeft',
	'MetaRight',
	'OSLeft',
	'OSRight'
]);

export function shortcutFromEvent(event: KeyboardEvent): string | null {
	if (MODIFIER_CODES.has(event.code) || event.code === 'Escape') return null;
	const parts: string[] = [];
	if (event.ctrlKey) parts.push('Ctrl');
	if (event.altKey) parts.push('Alt');
	if (event.shiftKey) parts.push('Shift');
	if (event.metaKey) parts.push('Super');
	if (!event.ctrlKey && !event.altKey && !event.metaKey) return null;
	parts.push(event.code);
	return parts.join('+');
}

export function normalizeShortcut(shortcut: string): string {
	return shortcut
		.split('+')
		.map((part) => part.trim())
		.filter(Boolean)
		.map((part) => {
			const lower = part.toLowerCase();
			if (lower === 'control' || lower === 'ctrl') return 'Ctrl';
			if (lower === 'alt' || lower === 'option') return 'Alt';
			if (lower === 'shift') return 'Shift';
			if (lower === 'meta' || lower === 'super' || lower === 'cmd' || lower === 'command')
				return 'Super';
			if (/^[a-z]$/i.test(part)) return `Key${part.toUpperCase()}`;
			if (/^\d$/.test(part)) return `Digit${part}`;
			return part;
		})
		.join('+');
}

export function shortcutMatches(event: KeyboardEvent, shortcut: string): boolean {
	const expected = normalizeShortcut(shortcut).split('+');
	const code = expected.at(-1);
	return (
		event.code === code &&
		event.ctrlKey === expected.includes('Ctrl') &&
		event.altKey === expected.includes('Alt') &&
		event.shiftKey === expected.includes('Shift') &&
		event.metaKey === expected.includes('Super')
	);
}

export function shortcutsCollide(left: string, right: string): boolean {
	return normalizeShortcut(left) === normalizeShortcut(right);
}

export function prettyShortcut(shortcut: string): string {
	return normalizeShortcut(shortcut)
		.replace(/\bKey([A-Z])\b/g, '$1')
		.replace(/\bDigit(\d)\b/g, '$1')
		.replace(/\+/g, ' + ');
}
