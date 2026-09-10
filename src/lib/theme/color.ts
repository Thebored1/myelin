import type { ThemeContrastResult, ThemeMode, ThemeTokenId, ThemeTokens } from './types';

const HEX_COLOR = /^#[0-9a-f]{6}([0-9a-f]{2})?$/i;

export function isHexColor(value: unknown): value is string {
	return typeof value === 'string' && HEX_COLOR.test(value);
}

export function normalizeHex(value: string, allowAlpha = true): string {
	const normalized = value.trim().toUpperCase();
	if (!isHexColor(normalized)) throw new Error(`Invalid theme color: ${value}`);
	if (!allowAlpha && normalized.length !== 7) {
		throw new Error(`Theme color does not allow transparency: ${value}`);
	}
	return normalized;
}

function channels(value: string): [number, number, number, number] {
	const normalized = normalizeHex(value);
	const alpha = normalized.length === 9 ? parseInt(normalized.slice(7), 16) / 255 : 1;
	return [
		parseInt(normalized.slice(1, 3), 16),
		parseInt(normalized.slice(3, 5), 16),
		parseInt(normalized.slice(5, 7), 16),
		alpha
	];
}

export function mixHex(first: string, second: string, amount: number): string {
	const a = channels(first);
	const b = channels(second);
	const t = Math.max(0, Math.min(1, amount));
	const values = a.map((channel, index) => channel + (b[index] - channel) * t);
	const alpha = Math.round(values[3] * 255)
		.toString(16)
		.padStart(2, '0')
		.toUpperCase();
	return `#${values
		.slice(0, 3)
		.map((value) => Math.round(value).toString(16).padStart(2, '0'))
		.join('')}${Math.round(values[3] * 255)
		.toString(16)
		.padStart(2, '0')}`
		.toUpperCase()
		.replace(/FF$/, alpha === 'FF' ? '' : 'FF');
}

export function relativeLuminance(value: string): number {
	const [r, g, b] = channels(value).map((channel) => channel / 255);
	const linear = (channel: number) =>
		channel <= 0.03928 ? channel / 12.92 : ((channel + 0.055) / 1.055) ** 2.4;
	return 0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b);
}

export function contrastRatio(first: string, second: string): number {
	const a = relativeLuminance(first);
	const b = relativeLuminance(second);
	const light = Math.max(a, b);
	const dark = Math.min(a, b);
	return (light + 0.05) / (dark + 0.05);
}

export function readableForeground(background: string): '#000000' | '#FFFFFF' {
	return contrastRatio(background, '#FFFFFF') >= contrastRatio(background, '#000000')
		? '#FFFFFF'
		: '#000000';
}

// ---- OKLCH color math ----------------------------------------------------
// Perceptually uniform conversions (Björn Ottosson's OKLab). Shifting
// lightness in OKLCH keeps the hue stable, which is what makes a single
// accent input able to drive a whole app-wide color family.

function srgbToLinear(channel: number): number {
	return channel <= 0.04045 ? channel / 12.92 : ((channel + 0.055) / 1.055) ** 2.4;
}

function linearToSrgb(channel: number): number {
	return channel <= 0.0031308 ? 12.92 * channel : 1.055 * channel ** (1 / 2.4) - 0.055;
}

function okLabToLinear(lightness: number, a: number, b: number): [number, number, number] {
	const l_ = lightness + 0.3963377774 * a + 0.2158037573 * b;
	const m_ = lightness - 0.1055613458 * a - 0.0638541728 * b;
	const s_ = lightness - 0.0894841775 * a - 1.291485548 * b;
	const l = l_ ** 3;
	const m = m_ ** 3;
	const s = s_ ** 3;
	return [
		4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
		-1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
		-0.0041960863 * l - 0.7034186147 * m + 1.707614701 * s
	];
}

function oklchToLinear(lightness: number, chroma: number, hue: number): [number, number, number] {
	const radians = (hue * Math.PI) / 180;
	return okLabToLinear(lightness, chroma * Math.cos(radians), chroma * Math.sin(radians));
}

export type Oklch = { l: number; c: number; h: number };

export function toOklch(value: string): Oklch {
	const [r, g, b] = channels(value)
		.slice(0, 3)
		.map((channel) => srgbToLinear(channel / 255));
	const l = 0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b;
	const m = 0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b;
	const s = 0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b;
	const l_ = Math.cbrt(l);
	const m_ = Math.cbrt(m);
	const s_ = Math.cbrt(s);
	const L = 0.2104542553 * l_ + 0.793617785 * m_ - 0.0040720468 * s_;
	const a = 1.9779984951 * l_ - 2.428592205 * m_ + 0.4505937099 * s_;
	const bChannel = 0.0259040371 * l_ + 0.7827717662 * m_ - 0.808675766 * s_;
	const chroma = Math.hypot(a, bChannel);
	return { l: L, c: chroma, h: ((Math.atan2(bChannel, a) * 180) / Math.PI + 360) % 360 };
}

function toHex(r: number, g: number, b: number): string {
	const channel = (value: number) =>
		Math.round(Math.max(0, Math.min(1, value)) * 255)
			.toString(16)
			.padStart(2, '0')
			.toUpperCase();
	return `#${channel(r)}${channel(g)}${channel(b)}`;
}

/** Rebuild a hex color from OKLCH, reducing chroma when needed to stay in
 *  gamut so the hue is never silently distorted. */
export function fromOklch(lightness: number, chroma: number, hue: number): string {
	const h = ((hue % 360) + 360) % 360;
	let c = Math.max(0, chroma);
	let [r, g, b] = oklchToLinear(lightness, c, h);
	while (c > 0 && (r < 0 || r > 1 || g < 0 || g > 1 || b < 0 || b > 1)) {
		c *= 0.9;
		[r, g, b] = oklchToLinear(lightness, c, h);
	}
	return toHex(linearToSrgb(r), linearToSrgb(g), linearToSrgb(b));
}

/** Shift OKLCH lightness by a fixed delta, keeping hue and chroma. */
export function adjustLightness(value: string, delta: number): string {
	const { l, c, h } = toOklch(value);
	return fromOklch(Math.max(0, Math.min(1, l + delta)), c, h);
}

/** Scale OKLCH chroma by a factor, keeping hue and lightness. */
export function adjustChroma(value: string, factor: number): string {
	const { l, c, h } = toOklch(value);
	return fromOklch(l, Math.max(0, c * factor), h);
}

/** Composite a foreground color over a fully opaque background. */
export function compositeOver(foreground: string, background: string, alpha: number): string {
	const fg = channels(foreground);
	const bg = channels(background);
	const t = Math.max(0, Math.min(1, alpha));
	const rgb = [0, 1, 2].map((index) => Math.round(fg[index] * t + bg[index] * (1 - t)));
	return `#${rgb
		.map((value) => value.toString(16).padStart(2, '0'))
		.join('')
		.toUpperCase()}`;
}

// ---- Accent family derivation ----------------------------------------------

export const accentDerivedTokenIds = [
	'accent-100',
	'accent-200',
	'accent-300',
	'accent-tint',
	'on-accent',
	'bg-selection',
	'text-selection'
] as const;

export const washDerivedTokenIds = [
	'bg-page',
	'bg-panel',
	'bg-elevated',
	'bg-input',
	'bg-modal',
	'border-default',
	'border-subtle'
] as const;

/** Derive the whole app-wide accent family from one input color. */
export function deriveAccentTokens(accent: string, mode: ThemeMode, panel: string): ThemeTokens {
	const base = normalizeHex(accent);
	const delta = mode === 'dark' ? 0.1 : -0.1;
	const selection = compositeOver(base, panel, 0.22);
	return {
		'accent-100': base,
		'accent-200': adjustLightness(base, delta),
		'accent-300': adjustChroma(adjustLightness(base, delta * 2), 0.85),
		'accent-tint': `${base.slice(0, 7)}1F`,
		'bg-selection': selection,
		'text-selection': readableForeground(selection),
		'on-accent': readableForeground(base)
	} as ThemeTokens;
}

export type WashSurfaces = {
	'bg-page': string;
	'bg-panel': string;
	'bg-elevated': string;
	'bg-input': string;
	'bg-modal': string;
	'border-default': string;
	'border-subtle': string;
};

/** Tint surfaces toward the accent hue at low chroma, keeping text readable.
 *  Achromatic accents leave the surfaces untouched. */
export function washSurfaces(
	accent: string,
	mode: ThemeMode,
	textPrimary: string,
	surfaces: WashSurfaces
): WashSurfaces {
	const accentOklch = toOklch(accent);
	if (accentOklch.c < 0.01) return surfaces;
	const baseChroma = mode === 'dark' ? 0.03 : 0.04;
	const tinted = (surface: string, chroma: number) =>
		fromOklch(toOklch(surface).l, chroma, accentOklch.h);
	const guarded = (surface: string, chroma: number): string => {
		let value = tinted(surface, chroma);
		while (contrastRatio(textPrimary, value) < 4.5 && chroma > 0.004) {
			chroma *= 0.8;
			value = tinted(surface, chroma);
		}
		return value;
	};
	return {
		'bg-page': guarded(surfaces['bg-page'], baseChroma),
		'bg-panel': guarded(surfaces['bg-panel'], baseChroma),
		'bg-elevated': tinted(surfaces['bg-elevated'], baseChroma),
		'bg-input': tinted(surfaces['bg-input'], baseChroma),
		'bg-modal': tinted(surfaces['bg-modal'], baseChroma),
		'border-default': tinted(surfaces['border-default'], baseChroma * 0.6),
		'border-subtle': tinted(surfaces['border-subtle'], baseChroma * 0.6)
	};
}

export function deriveThemeTokens(
	mode: ThemeMode,
	palette: Partial<{
		page: string;
		panel: string;
		text: string;
		mutedText: string;
		accent: string;
		danger: string;
		warning: string;
		success: string;
		info: string;
	}>
): ThemeTokens {
	const page = normalizeHex(palette.page ?? (mode === 'dark' ? '#000000' : '#F4F2EF'));
	const panel = normalizeHex(palette.panel ?? (mode === 'dark' ? '#0A0A0A' : '#FFFFFF'));
	const text = normalizeHex(palette.text ?? (mode === 'dark' ? '#EEEEEE' : '#1F1D1C'));
	const muted = normalizeHex(palette.mutedText ?? (mode === 'dark' ? '#A49D9A' : '#6E6A67'));
	const accent = normalizeHex(palette.accent ?? '#EF6F2E');
	const danger = normalizeHex(palette.danger ?? '#E05555');
	const warning = normalizeHex(palette.warning ?? '#E7B85C');
	const success = normalizeHex(palette.success ?? '#4CAF50');
	const info = normalizeHex(palette.info ?? '#64B5F6');
	const dark = '#000000';
	const light = '#FFFFFF';
	const textInverse = mode === 'dark' ? dark : light;
	const neutral =
		mode === 'dark'
			? [
					'#D6D3D2',
					'#CCC9C7',
					'#B8B3B0',
					'#A49D9A',
					'#8A8380',
					'#5C5855',
					'#4D4947',
					'#3D3A39',
					'#2E2C2B',
					'#1F1D1C'
				]
			: [
					'#1F1D1C',
					'#2E2C2B',
					'#3D3A39',
					'#4D4947',
					'#5C5855',
					'#8A8380',
					'#A49D9A',
					'#CCC9C7',
					'#DDD9D5',
					'#E8E4E0'
				];
	const overlay = mode === 'dark' ? '#FFFFFF0A' : '#0000000A';
	const strongOverlay = mode === 'dark' ? '#FFFFFF14' : '#00000012';
	const panelBlur = mode === 'dark' ? '#020202F0' : '#FFFFFFE6';
	const washed = washSurfaces(accent, mode, text, {
		'bg-page': page,
		'bg-panel': panel,
		'bg-elevated': mode === 'dark' ? '#262626' : light,
		'bg-input': panel,
		'bg-modal': mode === 'dark' ? '#151515' : light,
		'border-default': mode === 'dark' ? '#3D3A39' : '#E2DED9',
		'border-subtle': mode === 'dark' ? '#4D4947' : '#ECE9E5'
	});

	return {
		...deriveAccentTokens(accent, mode, panel),
		'surface-dark-primary': mode === 'dark' ? page : light,
		'surface-dark-secondary': mode === 'dark' ? panel : '#F4F2EF',
		'surface-light-primary': mode === 'dark' ? '#EEEEEE' : '#1F1D1C',
		'surface-light-secondary': mode === 'dark' ? '#FAFAFA' : '#2E2C2B',
		...Object.fromEntries(neutral.map((value, index) => [`neutral-${index + 1}00`, value])),
		'text-primary': text,
		'text-secondary': muted,
		'text-inverse': textInverse,
		'text-hero': mode === 'dark' ? '#F6F1E7' : '#1A1714',
		'text-muted': muted,
		'text-tertiary': muted,
		'text-error': mode === 'dark' ? '#FECACA' : '#B42318',
		'border-default': washed['border-default'],
		'border-subtle': washed['border-subtle'],
		'border-strong': mode === 'dark' ? '#5C5855' : '#C9C2BB',
		'bg-page': washed['bg-page'],
		'bg-panel': washed['bg-panel'],
		'bg-code': mode === 'dark' ? '#1F1D1C' : '#F0EDE9',
		'bg-elevated': washed['bg-elevated'],
		'bg-elevated-hover': mode === 'dark' ? '#333333' : '#F8F6F3',
		'bg-input': washed['bg-input'],
		'bg-modal': washed['bg-modal'],
		'bg-panel-blur': panelBlur,
		'hover-overlay': overlay,
		'hover-overlay-strong': strongOverlay,
		'overlay-faint': mode === 'dark' ? '#FFFFFF05' : '#00000006',
		scrim: mode === 'dark' ? '#00000099' : '#281F183F',
		'scrim-soft': mode === 'dark' ? '#00000080' : '#281F1833',
		'shadow-color': mode === 'dark' ? '#00000066' : '#3C32201F',
		'shadow-color-strong': mode === 'dark' ? '#000000CC' : '#3C322033',
		danger: danger,
		'danger-tint': `${danger.slice(0, 7)}1A`,
		'danger-text': mode === 'dark' ? '#FECACA' : '#B42318',
		'danger-bg': `${danger.slice(0, 7)}1F`,
		'danger-bg-strong': `${danger.slice(0, 7)}2E`,
		'danger-border': `${danger.slice(0, 7)}59`,
		success: success,
		'success-border': `${success.slice(0, 7)}4D`,
		'success-fill': `${success.slice(0, 7)}0F`,
		'warning-border': `${warning.slice(0, 7)}4D`,
		'warning-fill': `${warning.slice(0, 7)}1A`,
		'info-border': `${info.slice(0, 7)}4D`,
		'info-fill': `${info.slice(0, 7)}0F`,
		warning,
		info
	} as ThemeTokens;
}

export function contrastChecks(tokens: ThemeTokens): ThemeContrastResult[] {
	const pairs: Array<[ThemeTokenId, ThemeTokenId]> = [
		['text-primary', 'bg-page'],
		['text-primary', 'bg-panel'],
		['text-secondary', 'bg-panel'],
		['on-accent', 'accent-100'],
		['danger-text', 'danger-bg']
	];
	return pairs.map(([foreground, background]) => {
		const ratio = contrastRatio(tokens[foreground] ?? '#000000', tokens[background] ?? '#FFFFFF');
		return { foreground, background, ratio, pass: ratio >= 4.5 };
	});
}
