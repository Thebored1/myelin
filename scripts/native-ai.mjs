#!/usr/bin/env node
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { join, resolve } from "node:path";
import { spawnSync } from "node:child_process";

const args = process.argv.slice(2);
const value = (name) => {
	const i = args.indexOf(name);
	return i >= 0 ? args[i + 1] : undefined;
};
const flag = (name) => args.includes(name);
const iterations = Math.max(1, Number(value("--iterations") ?? 3));
const appData =
	process.env.MYELIN_APP_DATA ??
	(process.platform === "win32"
		? join(process.env.APPDATA ?? homedir(), "com.paper.myelin")
		: process.platform === "darwin"
			? join(homedir(), "Library", "Application Support", "com.paper.myelin")
			: join(process.env.XDG_DATA_HOME ?? join(homedir(), ".local", "share"), "com.paper.myelin"));
let configured = {};
try {
	configured = JSON.parse(readFileSync(join(appData, "llama-server.json"), "utf8"));
} catch {
	// Explicit CLI overrides remain sufficient.
}
const modelValue = value("--model") ?? configured.modelPath ?? configured.model_path;
const llamaBinValue =
	value("--llama-bin") ?? configured.executablePath ?? configured.executable_path;
const model = modelValue ? resolve(modelValue) : "";
const llamaBin = llamaBinValue ? resolve(llamaBinValue) : "";
if (!modelValue || !llamaBinValue || !existsSync(model) || !existsSync(llamaBin)) {
	const message =
		"native AI prerequisites missing; pass --model <gguf> and --llama-bin <llama-server>, or configure Myelin first";
	if (flag("--json")) console.log(JSON.stringify({ status: "unsupported", message }));
	else console.error(message);
	process.exit(2);
}

const stamp = new Date().toISOString().replaceAll(":", "-").replace(/\.\d+Z$/, "Z");
const artifactDir = resolve("artifacts", "native-ai", stamp);
mkdirSync(artifactDir, { recursive: true });
const requestedCase = value("--case");
const engine = value("--engine") ?? (llamaBin.toLowerCase().includes("bee") ? "beellama" : "llama_cpp");
const lfmTemplate = model.toLowerCase().includes("lfm2.5")
	? resolve("src-tauri/templates/lfm25.jinja")
	: model.toLowerCase().includes("lfm2")
		? resolve("src-tauri/templates/lfm2.jinja")
		: undefined;
const reproduction = [
	"npm run test:native-ai --",
	requestedCase ? `--case ${JSON.stringify(requestedCase)}` : "",
	`--engine ${engine}`,
	`--iterations ${iterations}`,
	`--model ${JSON.stringify(model)}`,
	`--llama-bin ${JSON.stringify(llamaBin)}`,
].filter(Boolean).join(" ");
const runs = [];
for (let i = 0; i < iterations; i += 1) {
	const port = 41000 + Math.floor(Math.random() * 15000);
	const child = spawnSync(
		"cargo",
		[
			"run", "--quiet", "--manifest-path", "src-tauri/Cargo.toml", "--bin", "tool_e2e", "--",
			model, llamaBin, String(port), requestedCase ?? "all",
		],
		{
			encoding: "utf8",
			timeout: 20 * 60_000,
			env: {
				...process.env,
				MYELIN_NATIVE_AI_ENGINE: engine,
				...(lfmTemplate ? { CHAT_TEMPLATE_FILE: lfmTemplate } : {}),
			},
		},
	);
	runs.push({
		iteration: i + 1,
		exitCode: child.status ?? 1,
		signal: child.signal,
		stdout: child.stdout,
		stderr: child.stderr,
	});
}
const passed = runs.filter((run) => run.exitCode === 0).length;
const required = Math.min(iterations, Math.max(1, Math.ceil(iterations * 2 / 3)));
const status = passed >= required ? "pass" : "fail";
const report = {
	status,
	case: requestedCase ?? "all",
	engine,
	iterations,
	passed,
	required,
	model,
	llamaBin,
	artifactDir,
	reproduction,
	runs,
};
writeFileSync(join(artifactDir, "result.json"), `${JSON.stringify(report, null, 2)}\n`);
writeFileSync(
	join(artifactDir, "summary.txt"),
	`Native AI: ${status.toUpperCase()}\nPassed: ${passed}/${iterations} (required ${required})\nReproduce: ${reproduction}\n`,
);
if (flag("--json")) console.log(JSON.stringify(report));
else console.log(`Native AI ${status}: ${passed}/${iterations}; artifacts: ${artifactDir}`);
if (status === "pass" && !flag("--keep-artifacts")) {
	// Artifacts intentionally remain available for diagnostics; the flag is
	// accepted for CLI compatibility and documents that callers may rely on it.
}
process.exit(status === "pass" ? 0 : 1);
