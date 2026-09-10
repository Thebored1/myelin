import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type { AppearanceSettings, ColorTheme } from './types';

export type ThemePort = {
	getAppearance(): Promise<AppearanceSettings>;
	saveTheme(theme: ColorTheme): Promise<AppearanceSettings>;
	deleteTheme(id: string): Promise<AppearanceSettings>;
	setActiveTheme(id: string): Promise<AppearanceSettings>;
	listenChanged(callback: (settings: AppearanceSettings) => void): Promise<UnlistenFn>;
};

const isTauri = () =>
	typeof window !== 'undefined' &&
	Boolean((window as Window & { __TAURI_INTERNALS__?: object }).__TAURI_INTERNALS__);

const localFallback: ThemePort = {
	async getAppearance() {
		return { schemaVersion: 1, activeThemeId: 'myelin-dark', customThemes: [] };
	},
	async saveTheme() {
		return this.getAppearance();
	},
	async deleteTheme() {
		return this.getAppearance();
	},
	async setActiveTheme() {
		return this.getAppearance();
	},
	async listenChanged() {
		return () => {};
	}
};

export const productionThemePort: ThemePort = {
	getAppearance: () =>
		isTauri()
			? invoke<AppearanceSettings>('get_appearance_settings')
			: localFallback.getAppearance(),
	saveTheme: (theme) =>
		isTauri()
			? invoke<AppearanceSettings>('save_color_theme', { theme })
			: localFallback.saveTheme(theme),
	deleteTheme: (id) =>
		isTauri()
			? invoke<AppearanceSettings>('delete_color_theme', { id })
			: localFallback.deleteTheme(id),
	setActiveTheme: (id) =>
		isTauri()
			? invoke<AppearanceSettings>('set_active_color_theme', { id })
			: localFallback.setActiveTheme(id),
	listenChanged: async (callback) => {
		if (!isTauri()) return () => {};
		return listen<AppearanceSettings>('appearance://theme_changed', (event) =>
			callback(event.payload)
		);
	}
};
