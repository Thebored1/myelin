import { onDestroy, onMount } from 'svelte';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { appCache } from '$lib/appCache';

/** Owns settings bootstrap, live backend events, and provider polling. */
export function createSettingsLifecycle(ctx: Record<string, any>) {
	const unlisteners: Array<() => void> = [];
	onDestroy(() => {
		if (ctx.statusPoll) clearInterval(ctx.statusPoll);
		for (const unlisten of unlisteners) unlisten();
	});

	onMount(async () => {
		try {
			await ctx.refreshAiConfig();
			await ctx.refreshSnapshot();
			const status = await ctx.loadProviderStatus();
			ctx.downloadableBackends = await invoke<string[]>('downloadable_backends');
			ctx.downloadableBeeBackends = await invoke<string[]>('downloadable_bee_backends');
			// Auto chooses based on hardware; the remaining values name the exact
			// backend. The retired generic GPU value is treated as Auto.
			ctx.backendPreference = ctx.normalizeBackendPreference(status.config?.backendPreference);
			ctx.thinking = status.config?.thinking ?? false;
			ctx.autoOffload = status.config?.autoOffload ?? true;
			ctx.deterministicTools = status.config?.deterministicTools ?? true;
			// Gating is opt-in and off by default (model-agnostic full toolset).
			ctx.toolGating = status.config?.toolGating ?? false;
			ctx.promptCache = status.config?.promptCache ?? true;
			ctx.recommendedThreads = status.recommendedThreads ?? null;
			try {
				ctx.llamaCache = await invoke('llama_cache_status');
			} catch (error) {
				console.error(error);
			}
			ctx.searxngUrl = (await invoke<string | null>('get_searxng_url')) ?? '';
			ctx.embedModelPath = (await invoke<string | null>('get_embed_model_path')) ?? '';
			ctx.quickShortcut = (await invoke<string>('get_quick_shortcut')) || 'Ctrl+Space';
			ctx.startWithSystem = (await invoke<{ startWithSystem: boolean }>('get_background_settings'))
				.startWithSystem;
			try {
				const openharn = await invoke<any>('get_openharn_settings');
				ctx.ohPort = openharn.port ?? null;
				ctx.ohBinPath = openharn.bin_path ?? '';
				ctx.ohToolMode =
					openharn.tool_mode === 'native' || openharn.tool_mode === 'prompt'
						? openharn.tool_mode
						: 'auto';
				// Manual grammar toggles only apply in explicit Prompt tools mode.
				ctx.ohStrict = ctx.ohToolMode === 'prompt' ? (openharn.strict ?? false) : false;
				ctx.ohPromptTools = false;
				ctx.ohCallOnly = ctx.ohToolMode === 'prompt' ? (openharn.call_only ?? false) : false;
				ctx.ohNoThink = openharn.no_think ?? false;
				ctx.ohToolChoice = openharn.tool_choice ?? '';
				ctx.ohTemplateKwargs = openharn.template_kwargs ?? '';
				ctx.ohMaxCalls = openharn.max_calls ?? null;
				ctx.ohTotalMax = openharn.total_max ?? null;
				ctx.ohToolTimeout = openharn.tool_timeout_secs ?? null;
				ctx.ohBaseUrl = openharn.base_url ?? '';
				ctx.externalEnabled = openharn.external_enabled ?? false;
				ctx.externalBaseUrl = openharn.external_base_url ?? '';
				ctx.externalModel = openharn.external_model ?? '';
				ctx.externalApiKey = openharn.external_api_key ?? '';
			} catch (error) {
				console.error('Failed to load openharn settings:', error);
			}
			try {
				ctx.modelProfiles = await invoke<any[]>('list_model_profiles');
			} catch {
				ctx.modelProfiles = [];
			}
			if (status.resolved) {
				ctx.currentModelPath = status.config?.modelPath || status.resolved.modelPath || '';
				ctx.contextSize = status.config?.contextSize ?? status.resolved.contextSize ?? null;
				ctx.gpuLayers = status.config?.gpuLayers ?? status.resolved.gpuLayers ?? null;
				ctx.threads = status.config?.threads ?? status.resolved.threads ?? null;
				ctx.temperature = status.config?.temperature ?? status.resolved.temperature ?? null;
				ctx.topP = status.config?.topP ?? status.resolved.topP ?? null;
				ctx.maxTurns = status.config?.maxTurns ?? null;
				ctx.extraArgs = status.config?.extraArgs ?? status.resolved.extraArgs ?? [];
			} else if (status.config) {
				ctx.currentModelPath = status.config.modelPath || '';
				ctx.contextSize = status.config.contextSize ?? null;
				ctx.gpuLayers = status.config.gpuLayers ?? null;
				ctx.threads = status.config.threads ?? null;
				ctx.temperature = status.config.temperature ?? null;
				ctx.topP = status.config.topP ?? null;
				ctx.maxTurns = status.config.maxTurns ?? null;
				ctx.extraArgs = status.config.extraArgs ?? [];
			}

			ctx.enableJupyterExecution = localStorage.getItem('myelin_jupyter_exec') === 'true';
			try {
				ctx.latexCache = await invoke('tectonic_cache_status');
			} catch (error) {
				console.error(error);
			}
			unlisteners.push(
				await listen<{ phase: string; bytes?: number; message?: string }>(
					'latex://download',
					async (event) => {
						const p = event.payload;
						if (p.phase === 'start') {
							ctx.latexDownloading = true;
							ctx.latexError = '';
							ctx.latexDownloadBytes = p.bytes ?? 0;
						} else if (p.phase === 'progress') {
							ctx.latexDownloading = true;
							ctx.latexDownloadBytes = p.bytes ?? ctx.latexDownloadBytes;
						} else if (p.phase === 'done') {
							ctx.latexDownloading = false;
							ctx.latexDownloadBytes = p.bytes ?? ctx.latexDownloadBytes;
							try {
								ctx.latexCache = await invoke('tectonic_cache_status');
							} catch (error) {
								console.error(error);
							}
						} else if (p.phase === 'error') {
							ctx.latexDownloading = false;
							ctx.latexError = p.message ?? 'Download failed';
						}
					}
				),
				await listen<{ backend: string; engine: 'llama_cpp' | 'beellama'; fellBackToCpu: boolean }>(
					'ai://llama_backend',
					(event) => {
						ctx.activeBackend = event.payload.backend;
						ctx.activeEngine = event.payload.engine;
						ctx.backendFellBack = event.payload.fellBackToCpu;
					}
				),
				await listen<{ backend: string; phase: string; percent: number; message: string }>(
					'backend://download',
					async (event) => {
						ctx.download = event.payload;
						if (event.payload.phase === 'done') {
							ctx.beeDownloadActive = false;
							await ctx.loadProviderStatus();
							setTimeout(() => {
								if (ctx.download?.phase === 'done') ctx.download = null;
							}, 4000);
						}
						if (event.payload.phase === 'error') ctx.beeDownloadActive = false;
					}
				)
			);

			ctx.statusPoll = setInterval(() => {
				ctx.loadProviderStatus().catch(() => {});
			}, 2500);
		} catch (error) {
			console.error('Failed to load provider status:', error);
		}
	});
}
