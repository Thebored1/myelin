import { describe, expect, it } from 'vitest';
import { providerAiStatus } from './aiStatus';

const resolved = {
	executablePath: '/bin/llama-server',
	modelPath: '/models/model.gguf',
	host: '127.0.0.1',
	port: 8080,
	contextSize: 4096,
	temperature: 0.7,
	topP: 0.9,
	extraArgs: []
};

describe('providerAiStatus', () => {
	it('reports an unconfigured provider when no model can be resolved', () => {
		expect(providerAiStatus({ healthy: false })).toBe('unconfigured');
	});

	it('reports an unavailable provider when a configured model cannot be resolved', () => {
		expect(providerAiStatus({ healthy: false, config: { modelPath: '/missing/model.gguf' } })).toBe(
			'unavailable'
		);
	});

	it('reports loading while the resolved model is not yet healthy', () => {
		expect(providerAiStatus({ healthy: false, resolved })).toBe('loading');
	});

	it('reports ready from model health without waiting for the full tool pipeline', () => {
		expect(providerAiStatus({ healthy: true, resolved })).toBe('ready');
	});

	it('recovers a prior failure once the model becomes healthy', () => {
		expect(providerAiStatus({ healthy: true, resolved }, 'unavailable')).toBe('ready');
	});
});
