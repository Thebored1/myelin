import { readFileSync, readdirSync, statSync } from 'node:fs';
import { join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(fileURLToPath(new URL('.', import.meta.url)), '..');

const facadeLimits = new Map([
	['src-tauri/src/state.rs', 200],
	['src-tauri/src/agent.rs', 150],
	['src-tauri/src/llama_server.rs', 150],
	['src/routes/notes/[id]/+page.svelte', 300],
	['src/routes/+page.svelte', 250],
	['src/routes/settings/+page.svelte', 200],
	['src/lib/components/PdfViewer.svelte', 300]
]);

// These are the extracted implementation roots. Build output and generated
// files are intentionally outside this guard.
const implementationRoots = [
	'src-tauri/src/state',
	'src-tauri/src/agent',
	'src-tauri/src/llama_server',
	'src-tauri/openharn-myelin/src',
	'src/lib/note-page/sessions',
	'src/lib/note-page/components',
	'src/lib/note-page',
	'src/lib/home',
	'src/lib/settings'
];
const excludedNames = new Set(['target', 'node_modules', '.svelte-kit', 'dist', 'build']);

// These files have documented reasons to remain larger than the normal module
// budget: compatibility facades preserve public bindings; the sidecar harness
// keeps its parser/grammar/context-fit corpus together; the runner keeps one
// ordered cancellation/tool-loop state machine; and the state modules each own
// a cross-cutting transaction or cache invariant that would be obscured by a
// purely mechanical split.
const documentedLargeFiles = new Set([
	'src/lib/home/controller.svelte.ts',
	'src/lib/settings/controller.svelte.ts',
	'src-tauri/openharn-myelin/src/harness.rs',
	'src-tauri/openharn-myelin/src/runner.rs',
	'src-tauri/openharn-myelin/src/runner/turn_loop.rs',
	'src/lib/note-page/controller.svelte.ts',
	'src-tauri/src/state/ai/section_cache.rs',
	'src-tauri/src/state/documents/helpers.rs',
	'src-tauri/src/state/retrieval.rs',
	'src-tauri/src/state/settings.rs'
]);

function lineCount(path) {
	return readFileSync(path, 'utf8').split(/\r?\n/).length - 1;
}

function walk(dir) {
	const entries = [];
	for (const name of readdirSync(dir)) {
		if (excludedNames.has(name)) continue;
		const path = join(dir, name);
		const stat = statSync(path);
		if (stat.isDirectory()) entries.push(...walk(path));
		else entries.push(path);
	}
	return entries;
}

const violations = [];
for (const [file, limit] of facadeLimits) {
	const path = join(root, file);
	const actual = lineCount(path);
	if (actual > limit) violations.push(`${file}: ${actual} lines (limit ${limit})`);
}

for (const rootDir of implementationRoots) {
	for (const path of walk(join(root, rootDir))) {
		const file = relative(root, path).replaceAll('\\', '/');
		if (documentedLargeFiles.has(file)) continue;
		if (!/\.(rs|ts|svelte|svelte\.ts)$/.test(file)) continue;
		if (/[/](tests?|fixtures|generated|assets)[/]/.test(file)) continue;
		const actual = lineCount(path);
		if (actual > 800) violations.push(`${file}: ${actual} lines (limit 800)`);
	}
}

if (violations.length) {
	console.error('Architecture size check failed:');
	for (const violation of violations) console.error(`- ${violation}`);
	process.exit(1);
}

console.log('Architecture size check passed.');
