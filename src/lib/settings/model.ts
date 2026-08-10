import type { ProviderStatus } from '$lib/types';
import type { BackendPref, InferenceForm, OpenharnForm, ProviderForm } from './types';

export function configMatchPositions(source: string, query: string): number[] {
	const needle = query.trim().toLowerCase();
	if (!needle) return [];
	const lower = source.toLowerCase();
	const positions: number[] = [];
	for (let offset = 0; offset < lower.length; ) {
		const found = lower.indexOf(needle, offset);
		if (found < 0) break;
		positions.push(found);
		offset = found + Math.max(needle.length, 1);
	}
	return positions;
}

export function backendPreference(
	value: NonNullable<ProviderStatus['config']>['backendPreference'] | undefined
): BackendPref {
	return value === 'cpu' || value === 'vulkan' || value === 'metal' || value === 'cuda' ? value : 'auto';
}

export function providerStatusToForm(status: ProviderStatus): ProviderForm {
	const config = status.config;
	const resolved = status.resolved;
	return {
		modelPath: config?.modelPath ?? resolved?.modelPath ?? '',
		contextSize: config?.contextSize ?? resolved?.contextSize ?? null,
		gpuLayers: config?.gpuLayers ?? resolved?.gpuLayers ?? null,
		threads: config?.threads ?? resolved?.threads ?? null,
		temperature: config?.temperature ?? resolved?.temperature ?? null,
		topP: config?.topP ?? resolved?.topP ?? null,
		maxTurns: config?.maxTurns ?? null,
		extraArgs: config?.extraArgs ?? resolved?.extraArgs ?? [],
		backendPreference: backendPreference(config?.backendPreference),
		thinking: config?.thinking ?? false,
		autoOffload: config?.autoOffload ?? true,
		inferenceEngine: status.configuredEngine ?? 'llama_cpp',
		recommendedThreads: status.recommendedThreads ?? null,
		activeBackend: status.activeBackend ?? resolved?.backend ?? null
	};
}

export function inferencePayload(form: InferenceForm) {
	const extraArgs = form.extraArgs.map((arg) => arg.trim()).filter(Boolean);
	return {
		contextSize: form.contextSize,
		gpuLayers: form.gpuLayers,
		threads: form.threads,
		temperature: form.temperature,
		topP: form.topP,
		extraArgs: extraArgs.length ? extraArgs : null,
		backendPreference: form.backendPreference,
		gpuDevice: null,
		thinking: form.thinking,
		autoOffload: form.autoOffload,
		maxTurns: form.maxTurns
	};
}

export function openharnPayload(form: OpenharnForm) {
	return {
		port: form.port || null,
		bin_path: form.binPath.trim() || null,
		tool_mode: form.toolMode,
		strict: form.strict,
		prompt_tools: form.promptTools,
		call_only: form.callOnly,
		no_think: form.noThink,
		tool_choice: form.toolChoice.trim() || null,
		template_kwargs: form.templateKwargs.trim() || null,
		max_calls: form.maxCalls || null,
		total_max: form.totalMax || null,
		tool_timeout_secs: form.toolTimeoutSecs || null,
		base_url: form.baseUrl.trim() || null,
		external_enabled: form.externalEnabled,
		external_base_url: form.externalBaseUrl.trim() || null,
		external_model: form.externalModel.trim() || null,
		external_api_key: form.externalApiKey.trim() || null
	};
}

export function recommendedBeeBackend(preference: BackendPref, downloadable: readonly string[], nvidia: boolean): string {
	if (preference === 'cpu') return 'cpu';
	if (downloadable.includes('metal')) return 'metal';
	if (nvidia && downloadable.includes('cuda')) return 'cuda';
	if (downloadable.includes('vulkan')) return 'vulkan';
	return 'cpu';
}
