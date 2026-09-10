import { get } from 'svelte/store';
import { ThemeController } from './theme/controller.svelte';

export type { AppearanceSettings, ColorTheme, ThemeMode, ThemeTokens } from './theme/types';

export const themeController = new ThemeController();
export const activeThemeId = themeController.activeThemeId;
export const availableThemes = themeController.themes;
export const activeColorTheme = themeController.activeTheme;
export const themeError = themeController.error;

// Compatibility store for existing consumers. New code should use the theme
// controller and activeColorTheme so custom themes are not reduced to light/dark.
export const theme = {
	subscribe(run: (value: 'light' | 'dark') => void) {
		return themeController.activeTheme.subscribe((value) => run(value.mode));
	}
};

export async function initializeThemes(): Promise<void> {
	await themeController.mount();
}

export function disposeThemes(): void {
	themeController.dispose();
}

export function toggleTheme(): void {
	const current = get(themeController.activeTheme);
	void themeController.activate(current.mode === 'dark' ? 'myelin-light' : 'myelin-dark');
}
