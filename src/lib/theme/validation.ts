import contract from '../../../schemas/theme-token-contract.v1.json';
import { isHexColor, normalizeHex } from './color';
import type { ColorTheme, ThemeTokens } from './types';

const themeNamePattern = /^[\p{L}\p{N}][\p{L}\p{N} _.-]{0,63}$/u;

export function validateTheme(theme: ColorTheme): string[] {
	const issues: string[] = [];
	if (!theme || typeof theme !== 'object') return ['Theme must be an object.'];
	if (theme.schemaVersion !== 1) issues.push('Unsupported theme schema version.');
	if (!theme.id || theme.id.length > 80) issues.push('Theme ID is invalid.');
	if (!themeNamePattern.test(theme.name)) issues.push('Theme name is invalid.');
	if (theme.mode !== 'dark' && theme.mode !== 'light') issues.push('Theme mode is invalid.');
	if (!theme.baseThemeId) issues.push('Theme base is missing.');

	if (theme.tokens !== undefined && (typeof theme.tokens !== 'object' || Array.isArray(theme.tokens))) {
		issues.push('Theme tokens must be an object.');
	}
	for (const [id, value] of Object.entries(theme.tokens ?? {})) {
		const definition = contract.tokens[id as keyof typeof contract.tokens];
		if (!definition) {
			issues.push(`Unknown theme token: ${id}`);
			continue;
		}
		if (!isHexColor(value)) {
			issues.push(`Theme token ${id} is not a hex color.`);
			continue;
		}
		if (!definition.alpha && value.length !== 7) {
			issues.push(`Theme token ${id} cannot contain transparency.`);
		}
	}

	if (theme.palette !== undefined && (typeof theme.palette !== 'object' || Array.isArray(theme.palette))) {
		issues.push('Theme palette must be an object.');
	}
	for (const [id, value] of Object.entries(theme.palette ?? {})) {
		if (!isHexColor(value)) issues.push(`Theme palette color ${id} is invalid.`);
	}
	return issues;
}

export function normalizeTheme(theme: ColorTheme): ColorTheme {
	const issues = validateTheme(theme);
	if (issues.length) throw new Error(issues.join(' '));
	const tokens: ThemeTokens = {};
	for (const [id, value] of Object.entries(theme.tokens ?? {})) {
		const definition = contract.tokens[id as keyof typeof contract.tokens];
		if (definition) tokens[id as keyof ThemeTokens] = normalizeHex(value, definition.alpha);
	}
	const palette = theme.palette
		? Object.fromEntries(Object.entries(theme.palette).map(([key, value]) => [key, normalizeHex(value)]))
		: undefined;
	return { ...theme, tokens, palette };
}
