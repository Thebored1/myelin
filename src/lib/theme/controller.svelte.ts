import { get, writable, type Writable } from 'svelte/store';
import { builtinTheme, builtinThemes } from './builtins';
import { cloneTheme, compileTheme } from './compiler';
import { cssVariableForToken } from './types';
import { normalizeTheme } from './validation';
import { productionThemePort, type ThemePort } from './port';
import type { AppearanceSettings, ColorTheme, ThemeTokens } from './types';

const ACTIVE_CACHE_KEY = 'myelin_theme_bootstrap_v1';
const LEGACY_KEY = 'myelin_theme';

export class ThemeController {
	readonly activeThemeId: Writable<string> = writable('myelin-dark');
	readonly themes: Writable<ColorTheme[]> = writable([...builtinThemes]);
	readonly activeTheme: Writable<ColorTheme> = writable(builtinThemes[0]);
	readonly previewTheme: Writable<ColorTheme | null> = writable(null);
	readonly error: Writable<string> = writable('');
	private unlisten: (() => void) | undefined;
	private initialized = false;

	constructor(private readonly port: ThemePort = productionThemePort) {
		this.applyTheme(builtinThemes[0]);
	}

	async mount(): Promise<void> {
		if (this.initialized) return;
		this.initialized = true;
		try {
			let settings = await this.port.getAppearance();
			// Migrate the pre-custom-theme local choice exactly once when the new
			// backend registry is still at its untouched dark default.
			if (settings.activeThemeId === 'myelin-dark' && settings.customThemes.length === 0) {
				const legacy = typeof localStorage !== 'undefined' ? localStorage.getItem(LEGACY_KEY) : null;
				if (legacy === 'light') settings = await this.port.setActiveTheme('myelin-light');
			}
			this.applySettings(settings);
			this.unlisten = await this.port.listenChanged((next) => this.applySettings(next));
		} catch (error) {
			this.error.set(error instanceof Error ? error.message : String(error));
		}
	}

	dispose(): void {
		this.unlisten?.();
		this.unlisten = undefined;
		this.initialized = false;
	}

	async activate(id: string): Promise<void> {
		try {
			const settings = await this.port.setActiveTheme(id);
			this.applySettings(settings);
			this.error.set('');
		} catch (error) {
			this.error.set(error instanceof Error ? error.message : String(error));
			throw error;
		}
	}

	async save(theme: ColorTheme, activate = true): Promise<void> {
		const normalized = normalizeTheme({
			...theme,
			readonly: false,
			updatedAt: new Date().toISOString()
		});
		try {
			const settings = await this.port.saveTheme(normalized);
			this.previewTheme.set(null);
			this.applySettings(settings);
			if (activate && settings.activeThemeId !== normalized.id) await this.activate(normalized.id);
			this.error.set('');
		} catch (error) {
			this.error.set(error instanceof Error ? error.message : String(error));
			throw error;
		}
	}

	async importTheme(theme: ColorTheme): Promise<ColorTheme> {
		const existingIds = new Set(get(this.themes).map((entry) => entry.id));
		const imported = normalizeTheme({
			...theme,
			id: existingIds.has(theme.id) || builtinTheme(theme.id) ? crypto.randomUUID() : theme.id,
			name: existingIds.has(theme.id) ? `${theme.name} Import` : theme.name,
			readonly: false
		});
		await this.save(imported);
		return imported;
	}

	async duplicate(id: string): Promise<ColorTheme> {
		const source = get(this.themes).find((theme) => theme.id === id);
		if (!source) throw new Error('Theme was not found.');
		const copy = cloneTheme(source);
		await this.save(copy);
		return copy;
	}

	async delete(id: string): Promise<void> {
		if (builtinTheme(id)) throw new Error('Built-in themes cannot be deleted.');
		const settings = await this.port.deleteTheme(id);
		this.applySettings(settings);
	}

	preview(theme: ColorTheme): void {
		this.previewTheme.set(theme);
		this.applyTheme(theme);
	}

	clearPreview(): void {
		this.previewTheme.set(null);
		this.applyTheme(get(this.activeTheme));
	}

	private applySettings(settings: AppearanceSettings): void {
		const custom = settings.customThemes.map((theme) => normalizeTheme(theme));
		const all = [...builtinThemes, ...custom];
		const active = all.find((theme) => theme.id === settings.activeThemeId) ?? builtinThemes[0];
		this.themes.set(all);
		this.activeThemeId.set(active.id);
		this.activeTheme.set(active);
		if (!get(this.previewTheme)) this.applyTheme(active);
	}

	private applyTheme(theme: ColorTheme): void {
		if (typeof document === 'undefined') return;
		const root = document.documentElement;
		const tokens: ThemeTokens = compileTheme(theme);
		root.dataset.theme = theme.mode;
		root.dataset.themeId = theme.id;
		root.style.setProperty('color-scheme', theme.mode);
		for (const [id, value] of Object.entries(tokens)) {
			root.style.setProperty(cssVariableForToken(id as keyof ThemeTokens), value);
		}
		try {
			localStorage.setItem(LEGACY_KEY, theme.mode);
			localStorage.setItem(
				ACTIVE_CACHE_KEY,
				JSON.stringify({ themeId: theme.id, mode: theme.mode, tokens })
			);
		} catch {
			// The backend remains authoritative; startup will recover on the next mount.
		}
	}
}
