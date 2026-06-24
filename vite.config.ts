import { defineConfig } from 'vitest/config';
import { sveltekit } from '@sveltejs/kit/vite';

export default defineConfig({
	plugins: [sveltekit()],
	// Dedicated dev port that tauri.conf.json's devUrl points at. strictPort makes
	// Vite FAIL LOUDLY if the port is taken instead of silently moving to 5174 —
	// which once let `tauri dev` load whatever app held 5173 (e.g. ggufplay) inside
	// myelin's window.
	server: { port: 1420, strictPort: true },
	test: {
		expect: { requireAssertions: true },
		projects: [
			{
				extends: './vite.config.ts',
				test: {
					name: 'server',
					environment: 'node',
					include: ['src/**/*.{test,spec}.{js,ts}'],
					exclude: ['src/**/*.svelte.{test,spec}.{js,ts}']
				}
			}
		]
	}
});
