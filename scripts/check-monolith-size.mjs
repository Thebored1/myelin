import { readFileSync, readdirSync, statSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
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
const normalLimit = 800;

// Baseline entries are a ratchet, not a permanent exemption: a file may never
// exceed its recorded budget, the budget may never be raised, and entries must
// be deleted once the file is back under the normal limit. See
// scripts/size-baseline.json for the full contract.
const baselinePath = 'scripts/size-baseline.json';
const baseline = JSON.parse(readFileSync(join(root, baselinePath), 'utf8'));
for (const key of Object.keys(baseline)) {
	if (key.startsWith('$')) delete baseline[key];
}

// Compare against the committed baseline so raised budgets are rejected even
// though the working-tree file matches them. Outside a git repo (or before the
// first commit) this guard is skipped and the working-tree baseline stands.
function committedBaseline() {
	try {
		const raw = execFileSync('git', ['show', `HEAD:${baselinePath}`], {
			cwd: root,
			encoding: 'utf8'
		});
		const parsed = JSON.parse(raw);
		for (const key of Object.keys(parsed)) {
			if (key.startsWith('$')) delete parsed[key];
		}
		return parsed;
	} catch {
		return null;
	}
}

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
		if (!/\.(rs|ts|svelte|svelte\.ts)$/.test(file)) continue;
		if (/[/](tests?|fixtures|generated|assets)[/]/.test(file)) continue;
		const actual = lineCount(path);
		const budget = baseline[file];
		if (budget !== undefined) {
			if (actual > budget) {
				violations.push(`${file}: ${actual} lines (ratchet budget ${budget})`);
			} else if (actual <= normalLimit) {
				violations.push(
					`${file}: ${actual} lines is under the normal ${normalLimit}-line limit; ` +
						'remove its stale size-baseline entry'
				);
			}
			continue;
		}
		if (actual > normalLimit) violations.push(`${file}: ${actual} lines (limit ${normalLimit})`);
	}
}

const committed = committedBaseline();
if (committed) {
	for (const [file, budget] of Object.entries(baseline)) {
		const previous = committed[file];
		if (previous !== undefined && budget > previous) {
			violations.push(
				`${baselinePath}: budget for ${file} was raised from ${previous} to ${budget}; ` +
					'ratchet budgets may only shrink'
			);
		}
	}
}

if (violations.length) {
	console.error('Architecture size check failed:');
	for (const violation of violations) console.error(`- ${violation}`);
	process.exit(1);
}

const ratcheted = Object.keys(baseline).length;
console.log('Architecture size check passed.');
if (ratcheted > 0) {
	console.log(
		`Size ratchet active: ${ratcheted} file(s) above the ${normalLimit}-line limit, ` +
			'budgets may only shrink.'
	);
}
