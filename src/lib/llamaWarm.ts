import { invoke } from '@tauri-apps/api/core';

// Tie the llama-server lifecycle to whether a note is open in the editor: keep it
// warm while a note is open (so the first chat is instant and later ones reuse
// the warm slot), and stop it — freeing RAM/VRAM — once the last note closes.
//
// A short grace period bridges leave→return navigation (and any transient
// double-mount) so we don't churn the model load. Switching note→note doesn't
// touch this at all: SvelteKit keeps the /notes/[id] component mounted and only
// updates the route param, so onMount/onDestroy fire just once per note session.

let openNotes = 0;
let stopTimer: ReturnType<typeof setTimeout> | null = null;
const STOP_GRACE_MS = 2500;

function cancelPendingStop() {
	if (stopTimer !== null) {
		clearTimeout(stopTimer);
		stopTimer = null;
	}
}

/** Call when a note view mounts: warm the server, cancel any pending shutdown. */
export function noteOpened(): void {
	openNotes += 1;
	cancelPendingStop();
	// Fire-and-forget — the first chat still cold-starts correctly if this races.
	invoke('warm_llama_server').catch(() => {});
}

/** Call when a note view unmounts: shut the server down once no note remains. */
export function noteClosed(): void {
	openNotes = Math.max(0, openNotes - 1);
	if (openNotes === 0) {
		cancelPendingStop();
		stopTimer = setTimeout(() => {
			stopTimer = null;
			if (openNotes === 0) invoke('stop_llama_server').catch(() => {});
		}, STOP_GRACE_MS);
	}
}
