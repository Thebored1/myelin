import { deriveThemeTokens } from './color';
import type { ColorTheme } from './types';

const now = '2026-01-01T00:00:00.000Z';

const darkTokens = {
	...deriveThemeTokens('dark', { accent: '#EF6F2E' }),
	'accent-tint': '#EE60181F',
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
