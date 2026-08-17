import type { ProviderStatus } from '$lib/types';

export type BackendPref = 'auto' | 'cuda' | 'vulkan' | 'metal' | 'cpu';
export type InferenceEngine = 'llama_cpp' | 'beellama';

export type InferenceForm = {
	contextSize: number | null;
	gpuLayers: number | null;
	threads: number | null;
	temperature: number | null;
	topP: number | null;
	maxTurns: number | null;
	extraArgs: string[];
	backendPreference: BackendPref;
	thinking: boolean;
	autoOffload: boolean;
};

export type OpenharnForm = {
	port: number | null;
	binPath: string;
	toolMode: 'auto' | 'native' | 'prompt';
	strict: boolean;
	promptTools: boolean;
	callOnly: boolean;
	noThink: boolean;
	toolChoice: string;
	templateKwargs: string;
	maxCalls: number | null;
	totalMax: number | null;
	toolTimeoutSecs: number | null;
	baseUrl: string;
	externalEnabled: boolean;
	externalBaseUrl: string;
	externalModel: string;
	externalApiKey: string;
};

export type ProviderForm = InferenceForm & {
	modelPath: string;
	inferenceEngine: InferenceEngine;
	recommendedThreads: number | null;
	activeBackend: string | null;
};

export type OcrStatus = {
	available?: boolean;
	languageAvailable?: boolean;
	version?: string;
	configuredPath?: string;
	configuredLanguage?: string;
	autoLowTextPages?: boolean;
	warning?: string;
	executablePath?: string;
};

export type { ProviderStatus };
