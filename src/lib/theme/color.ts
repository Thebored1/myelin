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
	const page = normalizeHex(palette.page ?? (mode === 'dark' ? '#020202' : '#F4F2EF'));
	const panel = normalizeHex(palette.panel ?? (mode === 'dark' ? '#101010' : '#FFFFFF'));
	const text = normalizeHex(palette.text ?? (mode === 'dark' ? '#EEEEEE' : '#1F1D1C'));
	const muted = normalizeHex(
		palette.mutedText ?? (mode === 'dark' ? '#A49D9A' : '#6E6A67')
	);
	const accent = normalizeHex(palette.accent ?? '#EF6F2E');
	const danger = normalizeHex(palette.danger ?? '#E05555');
	const warning = normalizeHex(palette.warning ?? '#E7B85C');
	const success = normalizeHex(palette.success ?? '#4CAF50');
	const info = normalizeHex(palette.info ?? '#64B5F6');
	const dark = '#000000';
	const light = '#FFFFFF';
	const textInverse = mode === 'dark' ? dark : light;
	const neutral = mode === 'dark'
		? ['#D6D3D2', '#CCC9C7', '#B8B3B0', '#A49D9A', '#8A8380', '#5C5855', '#4D4947', '#3D3A39', '#2E2C2B', '#1F1D1C']
		: ['#1F1D1C', '#2E2C2B', '#3D3A39', '#4D4947', '#5C5855', '#8A8380', '#A49D9A', '#CCC9C7', '#DDD9D5', '#E8E4E0'];
	const overlay = mode === 'dark' ? '#FFFFFF0A' : '#0000000A';
	const strongOverlay = mode === 'dark' ? '#FFFFFF14' : '#00000012';
	const panelBlur = mode === 'dark' ? '#101010F0' : '#FFFFFFE6';

	return {
		'accent-100': accent,
		'accent-200': normalizeHex(palette.accent ? mixHex(accent, mode === 'dark' ? dark : light, 0.08) : mode === 'dark' ? '#EE6018' : '#EE6018'),
		'accent-300': normalizeHex(palette.accent ? mixHex(accent, mode === 'dark' ? dark : light, 0.2) : '#D15010'),
		'surface-dark-primary': mode === 'dark' ? page : light,
		'surface-dark-secondary': mode === 'dark' ? panel : '#F4F2EF',
		'surface-light-primary': mode === 'dark' ? '#EEEEEE' : '#1F1D1C',
		'surface-light-secondary': mode === 'dark' ? '#FAFAFA' : '#2E2C2B',
		...Object.fromEntries(neutral.map((value, index) => [`neutral-${index + 1}00`, value])),
		'text-primary': text,
		'text-secondary': muted,
		'text-inverse': textInverse,
		'text-selection': mode === 'dark' ? dark : '#1F1D1C',
		'text-hero': mode === 'dark' ? '#F6F1E7' : '#1A1714',
		'text-muted': muted,
		'text-tertiary': muted,
		'text-error': mode === 'dark' ? '#FECACA' : '#B42318',
		'border-default': mode === 'dark' ? '#3D3A39' : '#E2DED9',
		'border-subtle': mode === 'dark' ? '#4D4947' : '#ECE9E5',
		'border-strong': mode === 'dark' ? '#5C5855' : '#C9C2BB',
		'bg-page': page,
		'bg-panel': panel,
		'bg-code': mode === 'dark' ? '#1F1D1C' : '#F0EDE9',
		'bg-selection': accent,
		'bg-elevated': mode === 'dark' ? '#262626' : light,
		'bg-elevated-hover': mode === 'dark' ? '#333333' : '#F8F6F3',
		'bg-input': panel,
		'bg-modal': mode === 'dark' ? '#151515' : light,
		'bg-panel-blur': panelBlur,
		'hover-overlay': overlay,
		'hover-overlay-strong': strongOverlay,
		'overlay-faint': mode === 'dark' ? '#FFFFFF05' : '#00000006',
		'scrim': mode === 'dark' ? '#00000099' : '#281F183F',
		'scrim-soft': mode === 'dark' ? '#00000080' : '#281F1833',
		'shadow-color': mode === 'dark' ? '#00000066' : '#3C32201F',
		'shadow-color-strong': mode === 'dark' ? '#000000CC' : '#3C322033',
		'accent-tint': `${accent.slice(0, 7)}1F`,
		'danger': danger,
		'danger-tint': `${danger.slice(0, 7)}1A`,
		'danger-text': mode === 'dark' ? '#FECACA' : '#B42318',
		'danger-bg': `${danger.slice(0, 7)}1F`,
		'danger-bg-strong': `${danger.slice(0, 7)}2E`,
		'danger-border': `${danger.slice(0, 7)}59`,
		'success': success,
		'success-border': `${success.slice(0, 7)}4D`,
		'success-fill': `${success.slice(0, 7)}0F`,
		'warning-border': `${warning.slice(0, 7)}4D`,
		'warning-fill': `${warning.slice(0, 7)}1A`,
		'info-border': `${info.slice(0, 7)}4D`,
		'info-fill': `${info.slice(0, 7)}0F`,
		'on-accent': readableForeground(accent),
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
