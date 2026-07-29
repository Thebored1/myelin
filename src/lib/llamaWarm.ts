import { invoke } from '@tauri-apps/api/core';

// Keep the llama-server warm for the entire app session. The server is started
// on app startup (see lib.rs setup hook) and lives until the app exits — there's
// no need to stop it when navigating between notes or closing the sidebar.
// The server process is killed automatically when the app closes (ManagedLlamaServer's
// Drop impl + shutdown_servers_sync in the close handler).
//
// noteOpened is kept as a harmless safety net: if the server somehow wasn't
// started at boot (e.g. a config issue fixed mid-session), opening a note will
// trigger the warm-up and subsequent chats will be fast.

/** Call when a note view mounts: warm the server if it isn't already. */
export type InteractionMode = 'chat' | 'operation' | 'edit';

export function noteOpened(noteId: string, interactionMode: InteractionMode = 'chat'): void {
	invoke('warm_llama_server', { noteId, interactionMode }).catch(() => {});
}

/** Called when a note view unmounts — no-op: the server stays warm. */
export function noteClosed(): void {
	// Server stays running for the whole app session.
}
