import { onMount } from 'svelte';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { getVersion } from '@tauri-apps/api/app';
import { appCache } from '$lib/appCache';
import type { StorageIssue } from '$lib/types';
import type { ControllerContext } from '$lib/controller-context';

/** Owns Home's bootstrap and event subscriptions. */
export function createHomeLifecycle(rawContext: object) {
	const ctx = rawContext as ControllerContext;
	onMount(() => {
		let unlistenChanged = () => {};
		let unlistenStatus = () => {};
		let unlistenTasks = () => {};
		let unlistenIssues = () => {};
		const hasTauriRuntime =
			typeof window !== 'undefined' &&
			Boolean((window as Window & { __TAURI_INTERNALS__?: object }).__TAURI_INTERNALS__);

		// Paint instantly from the last-known snapshot so coming back from a note
		// doesn't blank the UI while the backend responds.
		if (appCache.app) {
			ctx.app = appCache.app;
			ctx.provider = appCache.provider;
			ctx.appVersion = appCache.appVersion;
			ctx.indexing = appCache.app.indexState.isIndexing;
			ctx.ready = true;
		}

		// Vite's browser-only dev server does not provide Tauri IPC. Keep the
		// shell usable instead of leaving Home permanently stuck in its loading
		// branch when event.listen() cannot initialize.
		if (!hasTauriRuntime) {
			ctx.ready = true;
			ctx.message = 'Open Myelin through the Tauri desktop app to use a workspace.';
			return () => ctx.disposeTasks?.();
		}

		void (async () => {
			// Listen before bootstrap: startup indexing emits `notes_ready` as soon as
			// parsed notes can be opened, long before semantic indexing finishes.
			try {
				[unlistenChanged, unlistenStatus, unlistenTasks, unlistenIssues] = await Promise.all([
					listen('index://changed', () => {
						ctx.message = 'Reindexing…';
					}),
					listen<string>('index://status', (event) => {
						if (event.payload === 'started') {
							ctx.message = 'Loading your library…';
							ctx.indexing = true;
						} else if (event.payload === 'notes_ready') {
							ctx.message = 'Library ready — finishing search index…';
							ctx.indexing = true;
							void ctx.refreshApp();
						} else if (event.payload === 'completed') {
							ctx.message = '';
							ctx.indexing = false;
							void ctx.refreshApp();
						} else if (event.payload === 'failed') {
							ctx.message = 'Indexing failed. Try rebuilding the index.';
							ctx.indexing = false;
							void ctx.refreshApp();
						}
					}),
					listen<{ workspacePath?: string; source?: string }>('tasks://sync', (event) => {
						const ws = ctx.currentWorkspaceForTasks ?? ctx.app?.workspacePath;
						if (!ws) return;
						if (event.payload?.workspacePath && event.payload.workspacePath !== ws) return;
						if (event.payload?.source === 'main') return;
						void ctx.reloadTasks?.();
					}),
					listen<StorageIssue[]>('storage://issues', (event) => {
						if (ctx.app) ctx.app = { ...ctx.app, storageIssues: event.payload };
					})
				]);
			} catch (error) {
				console.error('Could not subscribe to native startup events', error);
			}

			if (!ctx.appVersion) {
				getVersion()
					.then((version) => {
						ctx.appVersion = version;
						appCache.appVersion = version;
					})
					.catch(() => {});
			}

			if (!appCache.app) {
				try {
					ctx.app = await invoke('get_snapshot');
					const snapshot = ctx.app;
					if (!snapshot) throw new Error('Snapshot was empty');
					ctx.provider = snapshot.providerStatus;
					ctx.indexing = snapshot.indexState.isIndexing;
					appCache.app = ctx.app;
					appCache.provider = ctx.provider;
				} catch (error) {
					console.error(error);
				} finally {
					ctx.ready = true;
				}
			}

			try {
				if (!appCache.bootstrapped) {
					ctx.indexing = true;
					ctx.app = await invoke('bootstrap');
					const snapshot = ctx.app;
					if (!snapshot) throw new Error('Bootstrap returned no snapshot');
					ctx.provider = snapshot.providerStatus;
					ctx.indexing = snapshot.indexState.isIndexing;
					appCache.bootstrapped = true;
					appCache.app = ctx.app;
				}
				await ctx.refreshApp();
			} catch (error) {
				console.error('Home startup failed', error);
			} finally {
				ctx.ready = true;
			}
		})();

		return () => {
			unlistenChanged();
			unlistenStatus();
			unlistenTasks();
			unlistenIssues();
			ctx.disposeTasks?.();
		};
	});
}
