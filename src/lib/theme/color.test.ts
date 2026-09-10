import { describe, expect, it } from 'vitest';
import {
	adjustChroma,
	adjustLightness,
	compositeOver,
	contrastRatio,
	deriveAccentTokens,
	deriveThemeTokens,
	fromOklch,
	isHexColor,
	mixHex,
	readableForeground,
	relativeLuminance,
	toOklch,
	washSurfaces
} from './color';
import { applyAccent, cloneTheme, compileTheme } from './compiler';
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

	it('round-trips colors through OKLCH without drifting', () => {
		for (const color of ['#000000', '#FFFFFF', '#EF6F2E', '#55AAFF', '#EAB308', '#101010']) {
			const { l, c, h } = toOklch(color);
			expect(fromOklch(l, c, h)).toBe(color);
		}
	});

	it('adjusts lightness while preserving hue', () => {
		const base = toOklch('#EF6F2E');
		const lighter = toOklch(adjustLightness('#EF6F2E', 0.1));
		const darker = toOklch(adjustLightness('#EF6F2E', -0.1));
		expect(lighter.l).toBeGreaterThan(base.l);
		expect(darker.l).toBeLessThan(base.l);
		expect(Math.abs(lighter.h - base.h)).toBeLessThan(1.5);
		expect(Math.abs(darker.h - base.h)).toBeLessThan(1.5);
	});

	it('scales chroma while keeping hue and lightness', () => {
		const base = toOklch('#EF6F2E');
		const reduced = toOklch(adjustChroma('#EF6F2E', 0.5));
		expect(reduced.c).toBeLessThan(base.c);
		expect(Math.abs(reduced.l - base.l)).toBeLessThan(0.002);
		expect(Math.abs(reduced.h - base.h)).toBeLessThan(1.5);
	});

	it('composites a foreground over an opaque background', () => {
		expect(compositeOver('#000000', '#FFFFFF', 0)).toBe('#FFFFFF');
		expect(compositeOver('#000000', '#FFFFFF', 1)).toBe('#000000');
		expect(compositeOver('#FFFFFF', '#000000', 0.5)).toBe('#808080');
	});

	it('derives the accent family from one input color', () => {
		const tokens = deriveAccentTokens('#55AAFF', 'dark', '#101010');
		expect(tokens['accent-100']).toBe('#55AAFF');
		expect(tokens['accent-tint']).toMatch(/^#55AAFF/);
		expect(contrastRatio(tokens['on-accent'] ?? '#FFFFFF', '#55AAFF')).toBeGreaterThanOrEqual(4.5);
		expect(
			contrastRatio(tokens['text-selection'] ?? '#000000', tokens['bg-selection'] ?? '#FFFFFF')
		).toBeGreaterThanOrEqual(4.5);
	});

	it('keeps surfaces readable when the wash cannot reach full contrast', () => {
		const text = '#EEEEEE';
		const washed = washSurfaces('#EAB308', 'dark', text, {
			'bg-page': '#020202',
			'bg-panel': '#101010',
			'bg-elevated': '#262626',
			'bg-input': '#101010',
			'bg-modal': '#151515',
			'border-default': '#3D3A39',
			'border-subtle': '#4D4947'
		});
		expect(contrastRatio(text, washed['bg-page'])).toBeGreaterThanOrEqual(4.5);
		expect(contrastRatio(text, washed['bg-panel'])).toBeGreaterThanOrEqual(4.5);
		expect(washed['bg-page']).not.toBe('#020202');
	});

	it('leaves surfaces untouched for achromatic accents', () => {
		const washed = washSurfaces('#777777', 'dark', '#EEEEEE', {
			'bg-page': '#020202',
			'bg-panel': '#101010',
			'bg-elevated': '#262626',
			'bg-input': '#101010',
			'bg-modal': '#151515',
			'border-default': '#3D3A39',
			'border-subtle': '#4D4947'
		});
		expect(washed['bg-page']).toBe('#020202');
		expect(washed['bg-panel']).toBe('#101010');
	});

	it('derives custom palette roles without arbitrary CSS', () => {
		const tokens = deriveThemeTokens('dark', { page: '#112233', accent: '#55AAFF' });
		expect(tokens['accent-100']).toBe('#55AAFF');
		expect(tokens['bg-page']).not.toBe('#112233');
		expect(toOklch(tokens['bg-page'] ?? '#000000').h).toBeCloseTo(toOklch('#55AAFF').h, 0);
		expect(Object.keys(tokens).every((key) => /^[a-z0-9-]+$/.test(key))).toBe(true);
	});

	it('applies a new accent to a theme without losing its neutrals', () => {
		const theme = applyAccent(darkTheme, '#EAB308');
		const tokens = compileTheme(theme);
		expect(tokens['accent-100']).toBe('#EAB308');
		expect(tokens['bg-page']).toMatch(/^#[0-9A-F]{6}$/);
		expect(theme.palette?.accent).toBe('#EAB308');
		const clone = cloneTheme(darkTheme);
		expect(compileTheme(clone)['accent-100']).toBe('#EF6F2E');
	});

	it('keeps the new surface direction for clones and any applied accent', () => {
		for (const base of [darkTheme, lightTheme]) {
			const original = compileTheme(base);
			const clone = compileTheme(cloneTheme(base));
			expect(clone['bg-page']).toBe(original['bg-page']);
			expect(clone['bg-panel']).toBe(original['bg-panel']);
		}
		const check = (
			tokens: Record<string, string | undefined>,
			original: Record<string, string | undefined>
		) => {
			expect(
				Math.abs(
					relativeLuminance(tokens['bg-page'] ?? '#000000') -
						relativeLuminance(original['bg-page'] ?? '#000000')
				)
			).toBeLessThan(0.02);
			expect(
				Math.abs(
					relativeLuminance(tokens['bg-panel'] ?? '#FFFFFF') -
						relativeLuminance(original['bg-panel'] ?? '#FFFFFF')
				)
			).toBeLessThan(0.02);
		};
		for (const base of [darkTheme, lightTheme]) {
			const original = compileTheme(base);
			const accent = compileTheme(applyAccent(base, '#EAB308'));
			if (base.mode === 'light') {
				expect(relativeLuminance(accent['bg-page'] ?? '#000000')).toBeGreaterThan(
					relativeLuminance(accent['bg-panel'] ?? '#FFFFFF')
				);
			} else {
				expect(relativeLuminance(accent['bg-page'] ?? '#000000')).toBeLessThan(
					relativeLuminance(accent['bg-panel'] ?? '#FFFFFF')
				);
			}
			check(accent, original);
		}
	});

	it('swaps main-area and menu-area surface colors globally', () => {
		const derived = deriveThemeTokens('light', {
			page: '#FFFFFF',
			panel: '#112233',
			accent: '#55AAFF'
		});
		expect(relativeLuminance(derived['bg-page'] ?? '#000000')).toBeGreaterThan(
			relativeLuminance(derived['bg-panel'] ?? '#FFFFFF')
		);
		const compiled = compileTheme({
			...darkTheme,
			mode: 'light',
			palette: { page: '#FFFFFF', panel: '#112233', accent: '#55AAFF' },
			tokens: {}
		});
		expect(compiled['bg-page']).not.toBe(compiled['bg-panel']);
		expect(relativeLuminance(compiled['bg-page'] ?? '#000000')).toBeLessThan(
			relativeLuminance(compiled['bg-panel'] ?? '#FFFFFF')
		);
		const explicit = compileTheme({
			...darkTheme,
			mode: 'light',
			tokens: { 'bg-page': '#FFFFFF', 'bg-panel': '#112233' }
		});
		expect(explicit['bg-page']).toBe('#112233');
		expect(explicit['bg-panel']).toBe('#FFFFFF');
	});

	it('keeps the dark menu area visibly lighter than the main surface', () => {
		const compiled = compileTheme(darkTheme);
		expect(relativeLuminance(compiled['bg-panel'] ?? '#000000')).toBeGreaterThan(
			3 * relativeLuminance(compiled['bg-page'] ?? '#FFFFFF')
		);
		expect(relativeLuminance(compiled['bg-elevated'] ?? '#000000')).toBeGreaterThan(
			relativeLuminance(compiled['bg-page'] ?? '#FFFFFF')
		);
	});

	it('keeps the light main surface visibly lighter than the menu area', () => {
		const compiled = compileTheme(lightTheme);
		expect(relativeLuminance(compiled['bg-page'] ?? '#000000')).toBeGreaterThan(
			relativeLuminance(compiled['bg-panel'] ?? '#FFFFFF')
		);
	});

	it('rejects unknown and malformed custom token values', () => {
		expect(() =>
			normalizeTheme({
				...darkTheme,
				id: 'custom',
				readonly: false,
				tokens: { 'not-a-token': '#FFFFFF' } as never
			})
		).toThrow();
	});
});
