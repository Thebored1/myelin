import { defineConfig } from 'vitest/config';
import { sveltekit } from '@sveltejs/kit/vite';
import { createLogger, type LogOptions } from 'vite';
import { resolve as resolvePath } from 'node:path';

const defaultLogger = createLogger();
const customLogger = {
	...defaultLogger,
	warn(message: string, options?: LogOptions) {
		if (
			message.includes('externalized for browser compatibility') &&
			message.includes('/node_modules/pyodide/')
		) {
			return;
		}
		defaultLogger.warn(message, options);
	}
};

export default defineConfig({
	plugins: [sveltekit()],
	customLogger,
	build: {
		// MathLive is an intentionally lazy, single-vendor chunk (~808 kB). The
		// PDF worker is emitted as a separate asset, so the default 500 kB
		// threshold reports a false-positive without improving the initial load.
		chunkSizeWarningLimit: 1024,
		rolldownOptions: {
			onwarn(warning, defaultHandler) {
				defaultHandler(warning);
			}
		}
	},
	// Dedicated dev port that tauri.conf.json's devUrl points at. strictPort makes
	// Vite FAIL LOUDLY if the port is taken instead of silently moving to 5174 —
	// which once let `tauri dev` load whatever app held 5173 (e.g. ggufplay) inside
	// myelin's window.
	server: {
		port: 1420,
		strictPort: true,
		// Theme types import the shared contract from the repository-level
		// schemas directory. SvelteKit narrows Vite's default allow-list to
		// source directories, so explicitly permit that contract in dev.
		fs: {
			allow: [resolvePath(process.cwd(), 'schemas')]
		},
		// Tauri's Rust build generates thousands of files under target/. Vite's
		// recursive watcher otherwise consumes the system inotify quota during
		// `tauri dev` and crashes with ENOSPC.
		watch: {
			ignored: ['**/src-tauri/target/**', '**/src-tauri/edit-core/target/**']
		}
	},
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
