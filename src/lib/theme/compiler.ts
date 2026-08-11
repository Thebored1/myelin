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
	// Direction contract: the main content area takes the dominant surface —
	// the lightest in light mode (panel) and the darkest in dark mode (page) —
	// while menus, rails and sidebars use the complementary surface. Enforced
	// once, after the full merge, so every theme — built-in, palette-derived,
	// or saved with explicit tokens — renders in the same direction. Stored
	// themes always carry the true surface roles; the pairing is applied at
	// compile time.
	const page = tokens['bg-page'];
	const panel = tokens['bg-panel'];
	if (normalized.mode === 'light' && page && panel) {
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
	// The palette is the pre-direction source of truth: deriveThemeTokens
	// expects the true page/panel roles. compileTheme only swaps those roles
	// for light mode, so the resolved surfaces recover the true roles either
	// directly (dark) or by un-swapping (light). Without this, re-deriving
	// from the crossed palette would flip the direction on the next compile.
	const swapped = theme.mode === 'light';
	const copy = makeCustomTheme(name, theme, {
		page: swapped ? resolved['bg-panel'] : resolved['bg-page'],
		panel: swapped ? resolved['bg-page'] : resolved['bg-panel'],
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
