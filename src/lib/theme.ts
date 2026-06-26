import { writable } from 'svelte/store';

// Light/dark theming. The actual colors live as CSS custom properties in
// +layout.svelte: `:root` holds the dark defaults, `:root[data-theme='light']`
// overrides them. This store just flips the `data-theme` attribute on <html>
// and remembers the choice. A tiny inline script in app.html applies the saved
// value before first paint so there's no dark-mode flash on light startup.

export type Theme = 'light' | 'dark';

const STORAGE_KEY = 'myelin_theme';

function readInitial(): Theme {
	if (typeof document !== 'undefined') {
		const attr = document.documentElement.dataset.theme;
		if (attr === 'light' || attr === 'dark') return attr;
	}
	if (typeof localStorage !== 'undefined') {
		const saved = localStorage.getItem(STORAGE_KEY);
		if (saved === 'light' || saved === 'dark') return saved;
	}
	return 'dark';
}

export const theme = writable<Theme>(readInitial());

// Mirror every change to the DOM and localStorage. writable runs this once
// immediately on subscribe, so importing this module is enough to apply it.
theme.subscribe((value) => {
	if (typeof document !== 'undefined') {
		document.documentElement.dataset.theme = value;
	}
	if (typeof localStorage !== 'undefined') {
		localStorage.setItem(STORAGE_KEY, value);
	}
});

export function toggleTheme(): void {
	theme.update((t) => (t === 'dark' ? 'light' : 'dark'));
}
