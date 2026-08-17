import { describe, expect, it } from 'vitest';
import { createBoundController } from './controllerView';

describe('createBoundController', () => {
	it('keeps state getters live and routes assignments to setters', () => {
		let count = 1;
		const view = createBoundController(
			() => ({ count }),
			{ count: (value) => (count = value) },
			{ increment: () => count++ }
		);

		expect(view.count).toBe(1);
		view.increment();
		expect(view.count).toBe(2);
		view.count = 7;
		expect(count).toBe(7);
	});
});
