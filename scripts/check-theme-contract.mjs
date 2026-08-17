import { readFileSync, readdirSync, statSync } from 'node:fs';
import { join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(fileURLToPath(new URL('.', import.meta.url)), '..');
const contractPath = join(root, 'schemas/theme-token-contract.v1.json');
const contract = JSON.parse(readFileSync(contractPath, 'utf8'));
const sourceRoots = [join(root, 'src/lib'), join(root, 'src/routes')];
const excluded = new Set(['node_modules', '.svelte-kit', 'dist', 'build']);

function walk(directory) {
	const files = [];
	for (const name of readdirSync(directory)) {
		if (excluded.has(name)) continue;
		const path = join(directory, name);
		const stat = statSync(path);
		if (stat.isDirectory()) files.push(...walk(path));
		else if (/\.(css|svelte|ts)$/.test(name)) files.push(path);
	}
	return files;
}

const files = sourceRoots.flatMap(walk);
const declared = new Set(Object.keys(contract.tokens).map((token) => `--${token}`));
const localOnly = new Set([
	'--sidebar-width',
	'--swatch-border',
	'--toolbar-icon-color',
	'--toolbar-icon-hover-color'
]);
const declarationPattern = /(--[a-zA-Z0-9_-]+)\s*:/g;
for (const path of files) {
	const source = readFileSync(path, 'utf8');
	for (const match of source.matchAll(declarationPattern)) declared.add(match[1]);
}

const used = new Map();
for (const path of files) {
	const source = readFileSync(path, 'utf8');
	for (const match of source.matchAll(/var\((--[a-zA-Z0-9_-]+)/g)) {
		const token = match[1];
		if (!used.has(token)) used.set(token, relative(root, path));
	}
}

const unknown = [...used.entries()]
	.filter(([token]) => !declared.has(token) && !localOnly.has(token))
	.map(([token, path]) => `${token} (${path})`);

if (unknown.length) {
	console.error('Theme contract check failed. Undeclared CSS variables:');
	for (const token of unknown) console.error(`- ${token}`);
	process.exit(1);
}

const required = Object.entries(contract.tokens)
	.filter(([, definition]) => definition.required)
	.map(([token]) => `--${token}`);
const missing = required.filter((token) => !declared.has(token));
if (missing.length) {
	console.error('Theme contract check failed. Missing required tokens:');
	for (const token of missing) console.error(`- ${token}`);
	process.exit(1);
}

console.log(
	`Theme contract check passed (${Object.keys(contract.tokens).length} registered tokens).`
);
