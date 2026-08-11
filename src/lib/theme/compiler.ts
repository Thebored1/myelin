import { builtinTheme, darkTheme } from './builtins';
import {
	accentDerivedTokenIds,
	deriveThemeTokens,
	normalizeHex,
	washDerivedTokenIds
} from './color';
import { normalizeTheme } from './validation';
import type { ColorTheme, ThemeTokens } from './types';

export function compileTheme(theme: ColorTheme): ThemeTokens {
	const normalized = normalizeTheme(theme);
	const base = builtinTheme(normalized.baseThemeId) ?? darkTheme;
	const derived = normalized.palette ? deriveThemeTokens(normalized.mode, normalized.palette) : {};
	const tokens = { ...base.tokens, ...derived, ...normalized.tokens };
	// The main content area sits on the panel color while menus, rails and
	// sidebars use the page color. Applied after the full merge so the
	// pairing holds for built-in and saved custom themes alike.
	const page = tokens['bg-page'];
	const panel = tokens['bg-panel'];
	if (page && panel) {
		tokens['bg-page'] = panel;
		tokens['bg-panel'] = page;
	}
	return tokens;
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

/** Rebuild a theme around one accent input: keep the resolved neutrals and
 *  status colors, clear accent/wash family overrides so the new accent fully
 *  drives the derived tokens. */
export function applyAccent(theme: ColorTheme, accent: string): ColorTheme {
	const draft = cloneTheme(theme);
	draft.palette = { ...draft.palette, accent: normalizeHex(accent) };
	for (const id of [...accentDerivedTokenIds, ...washDerivedTokenIds]) {
		delete draft.tokens[id];
	}
	return draft;
}
