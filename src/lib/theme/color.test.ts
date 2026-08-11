import { describe, expect, it } from 'vitest';
import { contrastRatio, deriveThemeTokens, isHexColor, mixHex, readableForeground } from './color';
import { compileTheme } from './compiler';
import { darkTheme, lightTheme } from './builtins';
import { normalizeTheme } from './validation';

describe('theme colors', () => {
	it('accepts only safe six or eight digit hex values', () => {
		expect(isHexColor('#AABBCC')).toBe(true);
		expect(isHexColor('#AABBCC80')).toBe(true);
		expect(isHexColor('red')).toBe(false);
		expect(isHexColor('#fff; background:url(https://example.com)')).toBe(false);
	});

	it('mixes colors deterministically', () => {
		expect(mixHex('#000000', '#FFFFFF', 0.5)).toBe('#808080');
	});

	it('chooses a readable accent foreground', () => {
		expect(readableForeground('#FFFFFF')).toBe('#000000');
		expect(readableForeground('#000000')).toBe('#FFFFFF');
		expect(contrastRatio('#000000', '#FFFFFF')).toBeCloseTo(21, 4);
	});

	it('compiles both built-in themes with required surfaces', () => {
		for (const theme of [darkTheme, lightTheme]) {
			const tokens = compileTheme(theme);
			expect(tokens['bg-page']).toMatch(/^#[0-9A-F]{6,8}$/);
			expect(tokens['bg-panel']).toMatch(/^#[0-9A-F]{6,8}$/);
			expect(tokens['text-primary']).toMatch(/^#[0-9A-F]{6,8}$/);
		}
	});

	it('derives custom palette roles without arbitrary CSS', () => {
		const tokens = deriveThemeTokens('dark', { page: '#112233', accent: '#55AAFF' });
		expect(tokens['bg-page']).toBe('#112233');
		expect(tokens['accent-100']).toBe('#55AAFF');
		expect(Object.keys(tokens).every((key) => /^[a-z0-9-]+$/.test(key))).toBe(true);
	});

	it('rejects unknown and malformed custom token values', () => {
		expect(() => normalizeTheme({ ...darkTheme, id: 'custom', readonly: false, tokens: { 'not-a-token': '#FFFFFF' } as never })).toThrow();
	});
});
