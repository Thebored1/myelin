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
	// A pure-black accent leaves a composited selection invisible on the
	// black surfaces, so selection uses an explicit monochrome gray.
	'bg-selection': '#333333',
	'text-selection': '#FFFFFF',
	'text-hero': '#FFFFFF',
	'accent-tint': '#0000001F',
	'danger-bg': '#EF44441F',
	'danger-bg-strong': '#EF44442E',
	'danger-border': '#EF444459',
	'success-border': '#81C7844D',
	'success-fill': '#81C7840F'
};

const lightTokens = {
	...deriveThemeTokens('light', { accent: '#EF6F2E' }),
	'accent-tint': '#EE60181A',
	'danger-bg': '#EF44441A',
	'danger-bg-strong': '#EF444428',
	'danger-border': '#EF444466'
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
	tokens: lightTokens,
	createdAt: now,
	updatedAt: now,
	readonly: true
};

export const builtinThemes = [darkTheme, lightTheme] as const;

export function builtinTheme(id: string): ColorTheme | undefined {
	return builtinThemes.find((theme) => theme.id === id);
}
