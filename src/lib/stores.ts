import { writable } from 'svelte/store';

// A writable that mirrors itself into localStorage, so the value survives reloads
// and app restarts. Guards against SSR / missing storage.
function persisted<T>(key: string, initial: T) {
	let start = initial;
	if (typeof localStorage !== 'undefined') {
		const saved = localStorage.getItem(key);
		if (saved !== null) {
			try {
				start = JSON.parse(saved) as T;
			} catch {
				/* ignore corrupt value */
			}
		}
	}
	const store = writable<T>(start);
	if (typeof localStorage !== 'undefined') {
		store.subscribe((v) => {
			try {
				localStorage.setItem(key, JSON.stringify(v));
			} catch {
				/* ignore quota/availability errors */
			}
		});
	}
	return store;
}

export const sidebarOpen = writable(true);
// Remembered across sessions (open by default the first time).
export const noteSidebarOpen = persisted('myelin_note_sidebar_open', true);
export const showSidebarToggle = writable(false);
