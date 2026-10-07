import { describe, expect, it, vi } from 'vitest';

/**
 * The note-page graph wires sessions together with *lazy* accessors: an early
 * session needs handlers owned by sessions constructed after it. Those accessors
 * must stay deferred.
 *
 * Spreading an object literal reads every getter immediately, which evaluates
 * the deferred ones against sessions that do not exist yet and throws a
 * TypeError. That failure happens during graph construction, so the whole note
 * page fails to mount and navigation into it silently does nothing — the kind
 * of bug no unit test on the sessions themselves would catch.
 */

/** A session-shaped object that does not exist yet, standing in for a cycle. */
type LateSession = { handleSectionsReady: () => void };

function installDeferred<T>(target: object, key: string, read: () => T) {
	Object.defineProperty(target, key, { enumerable: true, get: read });
}

describe('deferred session wiring', () => {
	it('throws if the accessor is read before its session exists', () => {
		// Guards the premise of this file: this really is a live TypeError.
		const slot: { current?: LateSession } = {};
		const view: Record<string, unknown> = { eager: 'ok' };
		installDeferred(view, 'handleSectionsReady', () => slot.current!.handleSectionsReady);
		expect(() => ({ ...view })).toThrow(TypeError);
	});

	it('survives building the view before the session is assigned', () => {
		// A mutable slot the graph fills in later, mirroring `let sourceSession`.
		const slot: { current?: LateSession } = {};
		const missingSession = (): LateSession => slot.current!;

		const view: Record<string, unknown> = { eager: 'ok' };
		installDeferred(view, 'handleSectionsReady', () => missingSession().handleSectionsReady);

		// Assignment by mutation — the pattern graph.svelte.ts must use. No throw.
		view.localVditorCdn = vi.fn();
		expect(() => view).not.toThrow();

		// The late session arrives and the accessor now resolves to it.
		const handler = vi.fn();
		slot.current = { handleSectionsReady: handler };
		const get = Object.getOwnPropertyDescriptor(view, 'handleSectionsReady')!.get!;
		expect(get.call(view)).toBe(handler);
	});
});
