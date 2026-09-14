import { deriveThemeTokens } from './color';
import type { ColorTheme } from './types';

const now = '2026-01-01T00:00:00.000Z';

const darkTokens = {
	...deriveThemeTokens('dark', {
		page: '#000000',
		panel: '#0A0A0A',
		text: '#EEEEEE',
		mutedText: '#A49D9A',
		accent: '#000000',
		danger: '#E05555',
		warning: '#E7885C',
		success: '#4CAF50',
		info: '#64B5F6'
	}),
	'text-hero': '#FFFFFF',
	'accent-tint': '#0000001F',
	'danger-bg': '#EF44441F',
	'danger-bg-strong': '#EF44442E',
	'danger-border': '#EF444459',
	'success-border': '#81C7844D',
	'success-fill': '#81C7840F'
};

// Keep the built-in light theme in sync with the neutral palette used by the
// user-facing "Myelin Light Copy" theme. This is deliberately achromatic:
// the previous orange accent also tinted the page gradient and made the whole
// light UI look pink/red even when no custom accent was selected.
const lightPalette = {
	page: '#F6F5F4',
	panel: '#FFFFFF',
	text: '#1F1D1C',
	mutedText: '#6E6A67',
	accent: '#FFFFFF',
	danger: '#E05555',
	warning: '#E7B85C',
	success: '#4CAF50',
	info: '#64B5F6'
} as const;

const lightTokens = {
	...deriveThemeTokens('light', lightPalette),
	'neutral-100': '#1F1D1C',
	'neutral-200': '#2E2C2B',
	'neutral-400': '#4D4947',
	'neutral-500': '#5C5855',
	'neutral-600': '#8A8380',
	'neutral-700': '#A49D9A',
	'neutral-800': '#CCC9C7',
	'neutral-900': '#DDD9D5',
	'neutral-1000': '#E8E4E0',
	'surface-light-primary': '#1F1D1C',
	'surface-light-secondary': '#2E2C2B',
	'surface-dark-secondary': '#F4F2EF',
	'bg-code': '#F0EDE9',
	'bg-elevated-hover': '#F8F6F3',
	'text-hero': '#1A1714',
	'danger-text': '#B42318',
	'hover-overlay': '#0000000A',
	'hover-overlay-strong': '#00000012',
	'overlay-faint': '#00000006',
	scrim: '#281F183F',
	'scrim-soft': '#281F1833',
	'shadow-color': '#3C32201F',
	'shadow-color-strong': '#3C322033',
	'danger-bg': '#EF44441A',
	'danger-bg-strong': '#EF444428',
	'danger-border': '#EF444466',
	'danger-tint': '#E055551A',
	'success-border': '#4CAF504D',
	'success-fill': '#4CAF500F',
	'warning-border': '#E7B85C4D',
	'warning-fill': '#E7B85C1A',
	'info-border': '#64B5F64D',
	'info-fill': '#64B5F60F',
	'bg-panel-blur': '#FFFFFFE6',
	'bg-elevated': '#FFFFFF'
};

export const darkTheme: ColorTheme = {
	schemaVersion: 1,
	id: 'myelin-dark',
	name: 'Myelin Dark',
	baseThemeId: 'myelin-dark',
	mode: 'dark',
	tokens: darkTokens,
	createdAt: now,
	updatedAt: now,
	readonly: true
};

export const lightTheme: ColorTheme = {
	schemaVersion: 1,
	id: 'myelin-light',
	name: 'Myelin Light',
	baseThemeId: 'myelin-light',
	mode: 'light',
	palette: lightPalette,
	tokens: lightTokens,
	createdAt: now,
	updatedAt: now,
	readonly: true
};

export const builtinThemes = [darkTheme, lightTheme] as const;

export function builtinTheme(id: string): ColorTheme | undefined {
	return builtinThemes.find((theme) => theme.id === id);
}
