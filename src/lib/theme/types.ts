import contract from '../../../schemas/theme-token-contract.v1.json';

export type ThemeMode = 'light' | 'dark';
export type ThemeTokenId = keyof typeof contract.tokens;
export type ThemeTokens = Partial<Record<ThemeTokenId, string>>;

export type ThemePalette = {
	page: string;
	panel: string;
	text: string;
	mutedText: string;
	accent: string;
	danger: string;
	warning: string;
	success: string;
	info: string;
};

export type ColorTheme = {
	schemaVersion: 1;
	id: string;
	name: string;
	baseThemeId: 'myelin-dark' | 'myelin-light';
	mode: ThemeMode;
	palette?: Partial<ThemePalette>;
	tokens: ThemeTokens;
	createdAt: string;
	updatedAt: string;
	readonly?: boolean;
};

export type AppearanceSettings = {
	schemaVersion: 1;
	activeThemeId: string;
	customThemes: ColorTheme[];
};

export type ThemeContrastResult = {
	foreground: ThemeTokenId;
	background: ThemeTokenId;
	ratio: number;
	pass: boolean;
};

export const themeTokenIds = Object.keys(contract.tokens) as ThemeTokenId[];
export const editableThemeTokenIds = themeTokenIds.filter(
	(id) => contract.tokens[id].editable
);

export function cssVariableForToken(id: ThemeTokenId): string {
	return `--${id}`;
}
