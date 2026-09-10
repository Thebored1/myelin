import { goto } from '$app/navigation';
import { resolve } from '$app/paths';
import type { NoteSummary } from '$lib/types';

type HomeNavigationPort = {
	get selectedNote(): NoteSummary | null;
	set selectedNote(value: NoteSummary | null);
	set tasksCollapsed(value: boolean);
	set activeMenuId(value: string | null);
	set noteToDelete(value: string | null);
	get deleteDialog(): HTMLDialogElement | undefined;
	get globalSearchDialog(): HTMLDialogElement | undefined;
	get filteredGlobalNotes(): NoteSummary[];
	get globalSelectedIndex(): number;
	set globalSelectedIndex(value: number);
};

export function createHomeNavigation(port: HomeNavigationPort) {
	async function openNote(noteId: string) {
		await goto(resolve(`/notes/${encodeURIComponent(noteId)}`));
	}

	function selectOrOpen(note: NoteSummary) {
		if (port.selectedNote?.id === note.id) void openNote(note.id);
		else {
			port.selectedNote = note;
			port.tasksCollapsed = true;
		}
	}

	function requestDeleteNote(event: MouseEvent, noteId: string) {
		event.stopPropagation();
		port.activeMenuId = null;
		port.noteToDelete = noteId;
		port.deleteDialog?.showModal();
	}

	function openNoteFromSearch(note: NoteSummary) {
		port.globalSearchDialog?.close();
		void openNote(note.id);
	}

	function handleGlobalSearchKeydown(event: KeyboardEvent) {
		if (event.key === 'ArrowDown') {
			event.preventDefault();
			if (port.globalSelectedIndex < port.filteredGlobalNotes.length - 1)
				port.globalSelectedIndex++;
		} else if (event.key === 'ArrowUp') {
			event.preventDefault();
			if (port.globalSelectedIndex > 0) port.globalSelectedIndex--;
		} else if (event.key === 'Enter') {
			event.preventDefault();
			const note = port.filteredGlobalNotes[port.globalSelectedIndex];
			if (note) openNoteFromSearch(note);
		} else if (event.key === 'Escape') {
			event.preventDefault();
			port.globalSearchDialog?.close();
		}
	}

	function autofocus(node: HTMLElement) {
		setTimeout(() => node.focus(), 10);
		return { destroy() {} };
	}

	function autoResize(node: HTMLTextAreaElement) {
		const resize = () => {
			node.style.height = 'auto';
			node.style.height = node.scrollHeight + 'px';
		};
		node.addEventListener('input', resize);
		setTimeout(resize, 0);
		return { destroy: () => node.removeEventListener('input', resize) };
	}

	return {
		openNote,
		selectOrOpen,
		requestDeleteNote,
		openNoteFromSearch,
		handleGlobalSearchKeydown,
		autofocus,
		autoResize
	};
}
