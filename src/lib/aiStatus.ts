import type { ProviderStatus } from './types';

export type AiStatus = 'loading' | 'ready' | 'unavailable' | 'unconfigured';

type ProviderReadiness = Pick<ProviderStatus, 'healthy' | 'config' | 'resolved'>;

export function providerAiStatus(
	status: ProviderReadiness,
	previous: AiStatus = 'loading'
): AiStatus {
	const hasModel = Boolean(status.config?.modelPath || status.resolved?.modelPath);
	if (!hasModel) return 'unconfigured';
	if (!status.resolved) return 'unavailable';
	// The titlebar reports whether the model itself is loaded. Full sidecar/tool
	// readiness remains available separately as ProviderStatus.ready.
	if (status.healthy) return 'ready';
	if (previous === 'unavailable') return 'unavailable';
	return 'loading';
}
