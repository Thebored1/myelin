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

// These are the extracted implementation roots. Build output, generated files,
// fixtures, and the transitional note composition controller are intentionally
// outside this guard: the controller remains the final coordination boundary
// and is being reduced through session slices.
const implementationRoots = [
	'src-tauri/src/state',
	'src-tauri/src/agent',
	'src-tauri/src/llama_server',
	'src/lib/note-page/sessions',
	'src/lib/note-page/components',
	'src/lib/home',
	'src/lib/settings'
];
const excludedNames = new Set(['target', 'node_modules', '.svelte-kit', 'dist', 'build']);
const excludedFiles = new Set([
	'src/lib/note-page/controller.svelte.ts'
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
		if (excludedFiles.has(file)) continue;
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
