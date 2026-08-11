import { builtinTheme, darkTheme } from './builtins';
import { deriveThemeTokens } from './color';
import { normalizeTheme } from './validation';
import type { ColorTheme, ThemeTokens } from './types';

export function compileTheme(theme: ColorTheme): ThemeTokens {
	const normalized = normalizeTheme(theme);
	const base = builtinTheme(normalized.baseThemeId) ?? darkTheme;
	const derived = normalized.palette ? deriveThemeTokens(normalized.mode, normalized.palette) : {};
	return { ...base.tokens, ...derived, ...normalized.tokens };
}

export function makeCustomTheme(
	name: string,
	base: ColorTheme,
	palette: ColorTheme['palette'] = {}
): ColorTheme {
	const now = new Date().toISOString();
	return {
		schemaVersion: 1,
		id: crypto.randomUUID(),
		name: name.trim(),
		baseThemeId: base.mode === 'light' ? 'myelin-light' : 'myelin-dark',
		mode: base.mode,
		palette,
		tokens: {},
		createdAt: now,
		updatedAt: now
	};
}

export function cloneTheme(theme: ColorTheme, name = `${theme.name} Copy`): ColorTheme {
	const resolved = compileTheme(theme);
	const copy = makeCustomTheme(name, theme, {
		page: resolved['bg-page'],
		panel: resolved['bg-panel'],
		text: resolved['text-primary'],
		mutedText: resolved['text-secondary'],
		accent: resolved['accent-100'],
		danger: resolved.danger,
		warning: resolved.warning,
		success: resolved.success,
		info: resolved.info
	});
	return { ...copy, tokens: { ...theme.tokens } };
}
