import type { AppSnapshot, ProviderStatus } from './types';

// In-memory cache that survives the Home page unmounting/remounting (SSR is off
// and the layout persists, so a module-level object outlives navigation). It lets
// returning from a note paint instantly from the last snapshot instead of blocking
// on the backend.
//
// `bootstrapped` guards the one-time heavy bootstrap (git init + file watcher +
// FULL workspace reindex). Once that's run for the session, later Home visits use
// the cheap in-memory get_snapshot and let the file watcher keep things fresh.
export const appCache: {
	app: AppSnapshot | null;
	provider: ProviderStatus | null;
	bootstrapped: boolean;
	appVersion: string;
} = {
	app: null,
	provider: null,
	bootstrapped: false,
	appVersion: ''
};
