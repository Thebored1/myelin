import { goto, beforeNavigate } from '$app/navigation';
import { resolve } from '$app/paths';
import type { GitCommit } from '$lib/types';
import { invoke } from '@tauri-apps/api/core';
import type { ControllerContext } from '$lib/controller-context';

/** Owns history previews and unsaved-navigation protection. */
export function createNavigationSession(rawContext: object) {
	const ctx = rawContext as ControllerContext;
	let isProgrammaticNavigation = $state(false);
	let pendingNavigationUrl = $state('');
	let pendingBack = $state(false);

	async function fetchNoteHistory() {
		if (!ctx.note) return;
		ctx.isBusy = true;
		try {
			const history = await invoke<GitCommit[]>('get_note_history', { noteId: ctx.note.id });
			ctx.noteHistory = history
				.filter((c) => c.message && c.message.trim() !== '')
				.sort((a, b) => Date.parse(b.timestamp) - Date.parse(a.timestamp));
		} catch (e) {
			console.error('Failed to fetch history:', e);
		} finally {
			ctx.isBusy = false;
		}
	}

	async function previewVersion(commitHash: string) {
		if (!ctx.note) return;
		ctx.isBusy = true;
		try {
			let rawContent = await invoke<string>('get_note_version', {
				noteId: ctx.note.id,
				commitHash
			});
			if (rawContent.match(/^---\r?\n/)) {
				const match = rawContent.match(/^---\r?\n[\s\S]*?\n---\r?\n/);
				if (match) rawContent = rawContent.slice(match[0].length);
			}
			ctx.versionPreviewContent = rawContent;
			ctx.versionPreviewHash = commitHash;
			ctx.versionPreviewDialog?.showModal();
		} catch (e) {
			console.error('Failed to fetch version:', e);
		} finally {
			ctx.isBusy = false;
		}
	}

	async function restoreVersion(commitHash: string) {
		if (!ctx.note) return;
		ctx.isBusy = true;
		try {
			let rawContent = await invoke<string>('get_note_version', {
				noteId: ctx.note.id,
				commitHash
			});
			if (rawContent.match(/^---\r?\n/)) {
				const match = rawContent.match(/^---\r?\n[\s\S]*?\n---\r?\n/);
				if (match) rawContent = rawContent.slice(match[0].length);
			}
			ctx.draftBody = rawContent;
			if (ctx.vditorInstance) ctx.vditorInstance.setValue(rawContent);
			ctx.versionPreviewContent = null;
			ctx.versionPreviewHash = null;
			ctx.versionPreviewDialog?.close();
			ctx.triggerAutoSave();
			ctx.activeSidebarTab = 'info';
		} catch (e) {
			console.error('Failed to restore version:', e);
		} finally {
			ctx.isBusy = false;
		}
	}

	function safeNavigate(url: string) {
		if (ctx.saveStatus === 'saving' || ctx.saveStatus === 'unsaved') {
			pendingNavigationUrl = url;
			ctx.navigationWarningDialog?.showModal();
			return;
		}
		isProgrammaticNavigation = true;
		// URL is supplied by the internal navigation guard or a trusted caller.
		// eslint-disable-next-line svelte/no-navigation-without-resolve
		void goto(url);
	}

	function navigateBack() {
		isProgrammaticNavigation = true;
		if (typeof window !== 'undefined' && window.history.length > 1) history.back();
		else void goto(resolve('/'));
	}

	function goBack() {
		if (ctx.hasReturnTo()) {
			safeNavigate(ctx.backUrl);
			return;
		}
		if (ctx.saveStatus === 'saving' || ctx.saveStatus === 'unsaved') {
			pendingBack = true;
			ctx.navigationWarningDialog?.showModal();
			return;
		}
		navigateBack();
	}

	function handleBeforeUnload(e: BeforeUnloadEvent) {
		if (isProgrammaticNavigation) return;
		if (ctx.saveStatus === 'saving' || ctx.saveStatus === 'unsaved') {
			e.preventDefault();
			e.returnValue = '';
		}
	}

	beforeNavigate(({ cancel, to }) => {
		if (isProgrammaticNavigation) return;
		if (ctx.saveStatus === 'saving' || ctx.saveStatus === 'unsaved') {
			pendingNavigationUrl = to?.url ? `${to.url.pathname}${to.url.search}${to.url.hash}` : '';
			ctx.navigationWarningDialog?.showModal();
			cancel();
		}
	});

	function confirmNavigation() {
		ctx.navigationWarningDialog?.close();
		if (pendingBack) {
			pendingBack = false;
			navigateBack();
			return;
		}
		if (pendingNavigationUrl) {
			isProgrammaticNavigation = true;
			// URL was captured from SvelteKit's beforeNavigate event.
			// eslint-disable-next-line svelte/no-navigation-without-resolve
			void goto(pendingNavigationUrl);
			pendingNavigationUrl = '';
		}
	}

	function cancelNavigation() {
		ctx.navigationWarningDialog?.close();
		pendingNavigationUrl = '';
		pendingBack = false;
	}

	return {
		fetchNoteHistory,
		previewVersion,
		restoreVersion,
		safeNavigate,
		goBack,
		navigateBack,
		handleBeforeUnload,
		confirmNavigation,
		cancelNavigation,
		get isProgrammaticNavigation() {
			return isProgrammaticNavigation;
		},
		set isProgrammaticNavigation(value) {
			isProgrammaticNavigation = value;
		},
		get pendingNavigationUrl() {
			return pendingNavigationUrl;
		},
		set pendingNavigationUrl(value) {
			pendingNavigationUrl = value;
		},
		get pendingBack() {
			return pendingBack;
		},
		set pendingBack(value) {
			pendingBack = value;
		}
	};
}
