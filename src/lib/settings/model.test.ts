import { describe, expect, it } from 'vitest';
import type { ProviderStatus } from '$lib/types';
import {
	configMatchPositions,
	inferencePayload,
	openharnPayload,
	providerStatusToForm,
	recommendedBeeBackend
} from './model';

const status: ProviderStatus = {
	activeProvider: 'llama.cpp',
	availableProviders: ['llama.cpp'],
	healthy: true,
	ready: true,
	detail: '',
	configuredEngine: 'beellama',
	recommendedThreads: 12,
	config: { modelPath: '', backendPreference: 'cpu', extraArgs: [' --flash ', ''], thinking: true },
	resolved: {
		executablePath: 'x',
		modelPath: '/model.gguf',
		host: '127.0.0.1',
		port: 1,
		contextSize: 4096,
		temperature: 0.2,
		topP: 0.9,
		extraArgs: [],
		backend: 'cpu'
	}
};

describe('settings model', () => {
	it('maps provider status with config precedence and defaults', () => {
		expect(providerStatusToForm(status)).toMatchObject({
			modelPath: '',
			contextSize: 4096,
			inferenceEngine: 'beellama',
			backendPreference: 'cpu'
		});
	});
	it('preserves command payload shapes while trimming optional values', () => {
		expect(
			inferencePayload({
				contextSize: null,
				gpuLayers: null,
				threads: null,
				temperature: null,
				topP: null,
				maxTurns: null,
				extraArgs: [' x ', ''],
				backendPreference: 'auto',
				thinking: false,
				autoOffload: true
			})
		).toMatchObject({ extraArgs: ['x'], gpuDevice: null });
		expect(
			openharnPayload({
				port: null,
				binPath: ' ',
				toolMode: 'prompt',
				strict: true,
				promptTools: false,
				callOnly: false,
				noThink: false,
				toolChoice: '',
				templateKwargs: '',
				maxCalls: null,
				totalMax: null,
				toolTimeoutSecs: null,
				baseUrl: '',
				externalEnabled: true,
				externalBaseUrl: ' https://example.test ',
				externalModel: ' m ',
				externalApiKey: ' secret '
			})
		).toMatchObject({
			external_enabled: true,
			external_base_url: 'https://example.test',
			external_api_key: 'secret'
		});
	});
	it('keeps search matches non-overlapping and chooses Bee defaults', () => {
		expect(configMatchPositions('aaaa', 'aa')).toEqual([0, 2]);
		expect(recommendedBeeBackend('auto', ['cuda', 'vulkan'], true)).toBe('cuda');
	});
});
