import { describe, expect, it } from 'vitest';
import {
	normalizeShortcut,
	prettyShortcut,
	shortcutMatches,
	shortcutsCollide
} from './keyboardShortcut';

describe('keyboard shortcuts', () => {
	it('normalizes and formats configured combinations', () => {
		expect(normalizeShortcut('Control+i')).toBe('Ctrl+KeyI');
		expect(prettyShortcut('Ctrl+KeyI')).toBe('Ctrl + I');
	});

	it('matches exact modifiers and physical key codes', () => {
		const event = { code: 'KeyI', ctrlKey: true, altKey: false, shiftKey: false, metaKey: false };
		expect(shortcutMatches(event as KeyboardEvent, 'Ctrl+KeyI')).toBe(true);
		expect(shortcutMatches(event as KeyboardEvent, 'Ctrl+Shift+KeyI')).toBe(false);
	});

	it('detects equivalent shortcut collisions', () => {
		expect(shortcutsCollide('Control+I', 'Ctrl+KeyI')).toBe(true);
		expect(shortcutsCollide('Ctrl+KeyI', 'Ctrl+Space')).toBe(false);
	});
});
