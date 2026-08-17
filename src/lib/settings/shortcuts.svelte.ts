import { invoke } from '@tauri-apps/api/core';
import { get } from 'svelte/store';
import { chatSidebarShortcut } from '$lib/stores';
import { shortcutFromEvent, shortcutsCollide } from '$lib/keyboardShortcut';
import type { ControllerContext } from '$lib/controller-context';

/** Owns quick-capture and chat-sidebar shortcut recording. */
export function createSettingsShortcuts(rawContext: object) {
	const ctx = rawContext as ControllerContext;
	async function applyShortcut(combo: string) {
		if (shortcutsCollide(combo, get(chatSidebarShortcut))) {
			ctx.quickShortcutError = 'This shortcut is already assigned to the chat sidebar.';
			return;
		}
		try {
			await invoke('set_quick_shortcut', { shortcut: combo });
			ctx.quickShortcut = combo;
			ctx.quickShortcutError = '';
		} catch (error) {
			ctx.quickShortcutError = String(error);
		}
	}

	function startRecording() {
		if (ctx.quickRecording) return;
		ctx.quickRecording = true;
		ctx.quickShortcutError = '';
		const modifiers = [
			'ControlLeft',
			'ControlRight',
			'AltLeft',
			'AltRight',
			'ShiftLeft',
			'ShiftRight',
			'MetaLeft',
			'MetaRight',
			'OSLeft',
			'OSRight'
		];
		const cleanup = () => {
			ctx.quickRecording = false;
			window.removeEventListener('keydown', onKey, true);
		};
		const onKey = (event: KeyboardEvent) => {
			event.preventDefault();
			event.stopPropagation();
			if (modifiers.includes(event.code)) return;
			if (event.code === 'Escape') {
				cleanup();
				return;
			}
			const parts: string[] = [];
			if (event.ctrlKey) parts.push('Ctrl');
			if (event.altKey) parts.push('Alt');
			if (event.shiftKey) parts.push('Shift');
			if (event.metaKey) parts.push('Super');
			parts.push(event.code);
			cleanup();
			void applyShortcut(parts.join('+'));
		};
		window.addEventListener('keydown', onKey, true);
	}

	function startChatShortcutRecording() {
		if (ctx.chatShortcutRecording) return;
		ctx.chatShortcutRecording = true;
		ctx.chatShortcutError = '';
		const modifiers = [
			'ControlLeft',
			'ControlRight',
			'AltLeft',
			'AltRight',
			'ShiftLeft',
			'ShiftRight',
			'MetaLeft',
			'MetaRight',
			'OSLeft',
			'OSRight'
		];
		const cleanup = () => {
			ctx.chatShortcutRecording = false;
			window.removeEventListener('keydown', onKey, true);
		};
		const onKey = (event: KeyboardEvent) => {
			event.preventDefault();
			event.stopPropagation();
			if (event.code === 'Escape') {
				cleanup();
				return;
			}
			const combo = shortcutFromEvent(event);
			if (!combo) {
				if (!modifiers.includes(event.code))
					ctx.chatShortcutError = 'Use Ctrl, Alt, or Super with another key.';
				return;
			}
			if (shortcutsCollide(combo, ctx.quickShortcut)) {
				ctx.chatShortcutError = 'This shortcut is already assigned to Quick Capture.';
				cleanup();
				return;
			}
			chatSidebarShortcut.set(combo);
			ctx.chatShortcutError = '';
			cleanup();
		};
		window.addEventListener('keydown', onKey, true);
	}

	return { applyShortcut, startRecording, startChatShortcutRecording };
}
