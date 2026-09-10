import { onDestroy } from 'svelte';
import { invoke } from '@tauri-apps/api/core';
import { open } from '@tauri-apps/plugin-dialog';
import { goto } from '$app/navigation';
import { resolve } from '$app/paths';
import type { AppSnapshot, IndexState, ProviderStatus } from '$lib/types';
import type { OcrStatus } from './types';
import {
	backendPreference as normalizeBackendPreference,
	recommendedBeeBackend as chooseBeeBackend
} from '$lib/settings/model';
import { createSettingsLifecycle } from './lifecycle.svelte';
import { createSettingsShortcuts } from './shortcuts.svelte';
import { createAiConfigController, type AiConfigStatus } from './ai-config';
import { createRetrievalController } from './retrieval';
import { createSettingsPersistence } from './persistence';
import { createBoundController } from '$lib/controllerView';
export function createSettingsController() {
	type BackendPref = 'auto' | 'cuda' | 'vulkan' | 'metal' | 'cpu';

	let currentModelPath = $state('');
	let contextSize = $state<number | null>(null);
	let gpuLayers = $state<number | null>(null);
	let threads = $state<number | null>(null);
	let recommendedThreads = $state<number | null>(null);
	let temperature = $state<number | null>(null);
	let topP = $state<number | null>(null);
	let maxTurns = $state<number | null>(null);
	let thinking = $state(false);
	let autoOffload = $state(true);
	let deterministicTools = $state(true);
	let toolGating = $state(false);
	let promptCache = $state(true);
	let llamaCache = $state<{ enabled: boolean; sizeBytes: number } | null>(null);
	let extraArgs = $state<string[]>([]);
	let activeWorkspacePath = $state('');
	let indexState = $state<IndexState | null>(null);
	let activeProvider = $state('');
	let inferenceEngine = $state<'llama_cpp' | 'beellama'>('llama_cpp');
	let activeEngine = $state<'llama_cpp' | 'beellama' | null>(null);
	let installedBeeBackends = $state<string[]>([]);
	let downloadableBeeBackends = $state<string[]>([]);
	let beeDownloadActive = $state(false);
	let backendPreference = $state<BackendPref>('auto');
	let downloadableBackends = $state<string[]>([]);
	let download = $state<{
		backend: string;
		phase: string;
		percent: number;
		message: string;
	} | null>(null);
	let activeBackend = $state<string | null>(null);
	let nvidiaDetected = $state(false);
	let gpuAvailable = $state(true);
	let gpus = $state<string[]>([]);
	let installedBackends = $state<string[]>([]);
	let backendFellBack = $state(false);
	let providerHealthy = $state(true);
	let providerDetail = $state('');
	// LaTeX → PDF support bundle (Tectonic) cache state.
	let latexCache = $state<{ warmed: boolean; sizeBytes: number } | null>(null);
	let latexDownloading = $state(false);
	let latexDownloadBytes = $state(0);
	let latexError = $state('');
	const formatMB = (bytes: number) => (bytes / (1024 * 1024)).toFixed(1) + ' MB';
	// Quick-capture global shortcut.
	let quickShortcut = $state('Ctrl+Space');
	let quickRecording = $state(false);
	let quickShortcutError = $state('');
	let chatShortcutRecording = $state(false);
	let chatShortcutError = $state('');
	let startWithSystem = $state(false);
	let backgroundError = $state('');
	const shortcutsPort = createBoundController(
		() => ({
			quickShortcut,
			quickRecording,
			quickShortcutError,
			chatShortcutRecording,
			chatShortcutError
		}),
		{
			quickRecording: (value) => (quickRecording = value),
			quickShortcutError: (value) => (quickShortcutError = value),
			chatShortcutRecording: (value) => (chatShortcutRecording = value),
			chatShortcutError: (value) => (chatShortcutError = value)
		},
		{}
	);
	const shortcuts = createSettingsShortcuts(shortcutsPort);
	const { applyShortcut, startRecording, startChatShortcutRecording } = shortcuts;
	const hasGpuBuild = () =>
		installedBackends.some((b) => b === 'cuda' || b === 'vulkan' || b === 'metal');
	const backendLabel = (b: string) =>
		b === 'cuda' ? 'CUDA' : b === 'vulkan' ? 'Vulkan' : b === 'metal' ? 'Metal' : 'CPU';
	const recommendedBeeBackend = $derived(
		chooseBeeBackend(backendPreference, downloadableBeeBackends, nvidiaDetected)
	);
	let statusPoll: ReturnType<typeof setInterval> | undefined;
	onDestroy(() => {
		if (statusPoll) clearInterval(statusPoll);
	});
	// Heads-up when the chosen GPU path isn't available / installed — the app
	// falls back to CPU automatically, so it's never a hard error.
	const gpuIssue = $derived.by((): { level: 'warn'; message: string } | null => {
		if (backendPreference === 'cpu') return null;
		if (!gpuAvailable) {
			return {
				level: 'warn',
				message: `No GPU detected${gpus.length ? ` (${gpus.join(', ')})` : ''} — running on CPU.`
			};
		}
		const need =
			backendPreference === 'vulkan'
				? 'vulkan'
				: backendPreference === 'metal'
					? 'metal'
					: backendPreference === 'cuda'
						? 'cuda'
						: nvidiaDetected
							? 'cuda'
							: 'vulkan';
		if (!installedBackends.includes(need)) {
			return {
				level: 'warn',
				message: `No ${backendLabel(need)} build installed — install it below, otherwise it runs on CPU.`
			};
		}
		return null;
	});
	// What the current selection resolves to, and whether it's live yet.
	const computeStatus = $derived.by((): { level: 'gpu' | 'cpu'; title: string; detail: string } => {
		const installed = (b: string) => installedBackends.includes(b);
		const target =
			backendPreference === 'cpu'
				? 'cpu'
				: backendPreference === 'vulkan'
					? installed('vulkan')
						? 'vulkan'
						: 'cpu'
					: backendPreference === 'metal'
						? installed('metal')
							? 'metal'
							: 'cpu'
						: backendPreference === 'cuda'
							? installed('cuda')
								? 'cuda'
								: 'cpu'
							: nvidiaDetected && installed('cuda')
								? 'cuda'
								: installed('vulkan')
									? 'vulkan'
									: installed('metal')
										? 'metal'
										: 'cpu';

		if (backendPreference !== 'cpu' && backendFellBack) {
			return {
				level: 'cpu',
				title: 'Running on CPU',
				detail: 'The GPU could not be used — check the GPU and driver.'
			};
		}
		if (target !== 'cpu' && activeBackend && activeBackend !== 'cpu') {
			return {
				level: 'gpu',
				title: `Running on ${activeBackend.toUpperCase()}`,
				detail: 'GPU acceleration active.'
			};
		}
		if (target === 'cpu') {
			const detail =
				backendPreference === 'cpu'
					? 'CPU mode — the most reliable option (works with every model).'
					: hasGpuBuild()
						? 'No GPU available on this machine.'
						: 'Install a GPU build below to accelerate.';
			return { level: 'cpu', title: 'Running on CPU', detail };
		}
		const pending = activeBackend !== null && activeBackend !== target;
		return {
			level: 'gpu',
			title: `Set to use ${target.toUpperCase()}`,
			detail: pending ? 'Applies on your next message.' : 'GPU acceleration active.'
		};
	});
	function selectBackend(value: BackendPref) {
		if (value === backendPreference) return;
		backendPreference = value;
		debounceSave();
	}
	let isSaving = $state(false);
	let isRebuilding = $state(false);
	let saved = $state(false);
	let enableJupyterExecution = $state(false);

	let ohPort = $state<number | null>(null);
	let ohBinPath = $state('');
	let ohToolMode = $state<'auto' | 'native' | 'prompt'>('auto');
	let ohStrict = $state(false);
	let ohPromptTools = $state(false);
	let ohCallOnly = $state(false);
	let ohNoThink = $state(false);
	let ohToolChoice = $state('');
	let ohTemplateKwargs = $state('');
	let ohMaxCalls = $state<number | null>(null);
	let ohTotalMax = $state<number | null>(null);
	let ohToolTimeout = $state<number | null>(null);
	let ohBaseUrl = $state('');
	let externalEnabled = $state(false);
	let externalBaseUrl = $state('');
	let externalModel = $state('');
	let externalApiKey = $state('');
	let ohSaving = $state(false);
	const persistencePort = createBoundController(
		() => ({
			currentModelPath,
			contextSize,
			gpuLayers,
			threads,
			temperature,
			topP,
			maxTurns,
			extraArgs,
			backendPreference,
			thinking,
			autoOffload,
			ohPort,
			ohBinPath,
			ohToolMode,
			ohStrict,
			ohPromptTools,
			ohCallOnly,
			ohNoThink,
			ohToolChoice,
			ohTemplateKwargs,
			ohMaxCalls,
			ohTotalMax,
			ohToolTimeout,
			ohBaseUrl,
			externalEnabled,
			externalBaseUrl,
			externalModel,
			externalApiKey,
			enableJupyterExecution,
			isSaving,
			saved,
			ohSaving
		}),
		{
			extraArgs: (value) => (extraArgs = value),
			ohBinPath: (value) => (ohBinPath = value),
			ohStrict: (value) => (ohStrict = value),
			ohPromptTools: (value) => (ohPromptTools = value),
			ohCallOnly: (value) => (ohCallOnly = value),
			isSaving: (value) => (isSaving = value),
			saved: (value) => (saved = value),
			ohSaving: (value) => (ohSaving = value),
			enableJupyterExecution: (value) => (enableJupyterExecution = value)
		},
		{}
	);
	const persistence = createSettingsPersistence(persistencePort);
	const {
		saveModelPath,
		saveAdvancedConfig,
		debounceSave,
		addExtraArg,
		removeExtraArg,
		toggleJupyterExecution,
		pickOpenharnBin,
		changeToolMode,
		saveOpenharn
	} = persistence;
	let aiConfig = $state<AiConfigStatus | null>(null);
	let aiConfigBusy = $state(false);
	let aiConfigMessage = $state('');
	let showAiConfig = $state(false);
	let aiConfigText = $state('');
	let aiConfigSearch = $state('');
	let aiConfigSearchInput: HTMLInputElement;
	let aiConfigEditor: HTMLTextAreaElement;
	let aiConfigSearchIndex = $state(-1);
	function handleAiConfigKeydown(event: KeyboardEvent) {
		if (!showAiConfig || !(event.ctrlKey || event.metaKey) || event.key.toLowerCase() !== 'f')
			return;
		event.preventDefault();
		event.stopPropagation();
		aiConfigSearchInput?.focus();
		aiConfigSearchInput?.select();
	}
	const aiConfigControllerPort = createBoundController(
		() => ({
			aiConfig,
			aiConfigBusy,
			aiConfigMessage,
			showAiConfig,
			aiConfigText,
			aiConfigSearch,
			aiConfigSearchInput,
			aiConfigEditor,
			aiConfigSearchIndex
		}),
		{
			aiConfig: (value) => (aiConfig = value),
			aiConfigBusy: (value) => (aiConfigBusy = value),
			aiConfigMessage: (value) => (aiConfigMessage = value),
			showAiConfig: (value) => (showAiConfig = value),
			aiConfigText: (value) => (aiConfigText = value),
			aiConfigSearchIndex: (value) => (aiConfigSearchIndex = value)
		},
		{}
	);
	const aiConfigController = createAiConfigController(aiConfigControllerPort);
	const {
		gotoConfigMatch,
		handleConfigSearchKeydown,
		configMatchCount,
		refreshAiConfig,
		validateAiConfig,
		applyAiConfig,
		copyAiConfigPath,
		openAiConfig,
		saveAiConfigText
	} = aiConfigController;
	// Web search + embeddings/RAG + model compatibility (Phase 5).
	let searxngUrl = $state('');
	let embedModelPath = $state('');
	let rerankerModelPath = $state('');
	let modelDownloadError = $state('');
	let ocrStatus = $state<OcrStatus | null>(null);
	let ocrSaving = $state(false);
	let downloadingModel = $state('');
	type ProfileInfo = {
		name: string;
		architecture?: string;
		namePattern?: string;
		role?: string;
		verified: boolean;
		notes?: string;
		supportsTools?: boolean;
	};
	let modelProfiles = $state<ProfileInfo[]>([]);

	const retrievalControllerPort = createBoundController(
		() => ({
			searxngUrl,
			embedModelPath,
			rerankerModelPath,
			modelDownloadError,
			downloadingModel,
			ocrSaving,
			ocrStatus
		}),
		{
			embedModelPath: (value) => (embedModelPath = value),
			rerankerModelPath: (value) => (rerankerModelPath = value),
			modelDownloadError: (value) => (modelDownloadError = value),
			downloadingModel: (value) => (downloadingModel = value),
			ocrSaving: (value) => (ocrSaving = value),
			ocrStatus: (value) => (ocrStatus = value)
		},
		{}
	);
	const retrievalController = createRetrievalController(retrievalControllerPort);
	const {
		saveSearxng,
		pickEmbedModel,
		clearEmbedModel,
		pickRerankerModel,
		clearRerankerModel,
		downloadBuiltInModel,
		saveOcrSettings
	} = retrievalController;

	async function refreshSnapshot() {
		const snapshot = await invoke<AppSnapshot>('get_snapshot');
		activeWorkspacePath = snapshot.workspacePath || '';
		indexState = snapshot.indexState ?? null;
	}

	// Refresh just the hardware/backend status fields (used after a download).
	async function loadProviderStatus(): Promise<ProviderStatus> {
		const status = await invoke<ProviderStatus>('get_provider_status');
		activeProvider = status.activeProvider || '';
		activeBackend = status.activeBackend ?? status.resolved?.backend ?? null;
		inferenceEngine = status.configuredEngine === 'beellama' ? 'beellama' : 'llama_cpp';
		activeEngine = status.activeEngine ?? status.resolved?.inferenceEngine ?? null;
		nvidiaDetected = status.nvidiaDetected ?? false;
		gpuAvailable = status.gpuAvailable ?? true;
		gpus = status.gpus ?? [];
		installedBackends = status.installedBackends ?? [];
		installedBeeBackends = status.installedBeeBackends ?? [];
		providerHealthy = status.healthy ?? true;
		providerDetail = status.detail ?? '';
		return status;
	}

	// Back: return to the page the user came from, not always home.
	function goBack() {
		if (typeof window !== 'undefined' && window.history.length > 1) {
			history.back();
		} else {
			goto(resolve('/'));
		}
	}

	const lifecyclePort = createBoundController(
		() => ({
			downloadableBackends,
			downloadableBeeBackends,
			backendPreference,
			thinking,
			autoOffload,
			deterministicTools,
			toolGating,
			promptCache,
			recommendedThreads,
			llamaCache,
			searxngUrl,
			embedModelPath,
			rerankerModelPath,
			modelDownloadError,
			ocrStatus,
			ocrSaving,
			downloadingModel,
			quickShortcut,
			startWithSystem,
			ohPort,
			ohBinPath,
			ohToolMode,
			ohStrict,
			ohPromptTools,
			ohCallOnly,
			ohNoThink,
			ohToolChoice,
			ohTemplateKwargs,
			ohMaxCalls,
			ohTotalMax,
			ohToolTimeout,
			ohBaseUrl,
			externalEnabled,
			externalBaseUrl,
			externalModel,
			externalApiKey,
			modelProfiles,
			currentModelPath,
			contextSize,
			gpuLayers,
			threads,
			temperature,
			topP,
			maxTurns,
			extraArgs,
			enableJupyterExecution,
			latexCache,
			latexDownloading,
			latexDownloadBytes,
			latexError,
			activeBackend,
			activeEngine,
			backendFellBack,
			beeDownloadActive,
			download,
			statusPoll
		}),
		{
			downloadableBackends: (value) => (downloadableBackends = value),
			downloadableBeeBackends: (value) => (downloadableBeeBackends = value),
			backendPreference: (value) => (backendPreference = value),
			thinking: (value) => (thinking = value),
			autoOffload: (value) => (autoOffload = value),
			deterministicTools: (value) => (deterministicTools = value),
			toolGating: (value) => (toolGating = value),
			promptCache: (value) => (promptCache = value),
			recommendedThreads: (value) => (recommendedThreads = value),
			llamaCache: (value) => (llamaCache = value),
			searxngUrl: (value) => (searxngUrl = value),
			embedModelPath: (value) => (embedModelPath = value),
			rerankerModelPath: (value) => (rerankerModelPath = value),
			modelDownloadError: (value) => (modelDownloadError = value),
			ocrStatus: (value) => (ocrStatus = value),
			ocrSaving: (value) => (ocrSaving = value),
			downloadingModel: (value) => (downloadingModel = value),
			quickShortcut: (value) => (quickShortcut = value),
			startWithSystem: (value) => (startWithSystem = value),
			ohPort: (value) => (ohPort = value),
			ohBinPath: (value) => (ohBinPath = value),
			ohToolMode: (value) => (ohToolMode = value),
			ohStrict: (value) => (ohStrict = value),
			ohPromptTools: (value) => (ohPromptTools = value),
			ohCallOnly: (value) => (ohCallOnly = value),
			ohNoThink: (value) => (ohNoThink = value),
			ohToolChoice: (value) => (ohToolChoice = value),
			ohTemplateKwargs: (value) => (ohTemplateKwargs = value),
			ohMaxCalls: (value) => (ohMaxCalls = value),
			ohTotalMax: (value) => (ohTotalMax = value),
			ohToolTimeout: (value) => (ohToolTimeout = value),
			ohBaseUrl: (value) => (ohBaseUrl = value),
			externalEnabled: (value) => (externalEnabled = value),
			externalBaseUrl: (value) => (externalBaseUrl = value),
			externalModel: (value) => (externalModel = value),
			externalApiKey: (value) => (externalApiKey = value),
			modelProfiles: (value) => (modelProfiles = value),
			currentModelPath: (value) => (currentModelPath = value),
			contextSize: (value) => (contextSize = value),
			gpuLayers: (value) => (gpuLayers = value),
			threads: (value) => (threads = value),
			temperature: (value) => (temperature = value),
			topP: (value) => (topP = value),
			maxTurns: (value) => (maxTurns = value),
			extraArgs: (value) => (extraArgs = value),
			enableJupyterExecution: (value) => (enableJupyterExecution = value),
			latexCache: (value) => (latexCache = value),
			latexDownloading: (value) => (latexDownloading = value),
			latexDownloadBytes: (value) => (latexDownloadBytes = value),
			latexError: (value) => (latexError = value),
			activeBackend: (value) => (activeBackend = value),
			activeEngine: (value) => (activeEngine = value),
			backendFellBack: (value) => (backendFellBack = value),
			beeDownloadActive: (value) => (beeDownloadActive = value),
			download: (value) => (download = value),
			statusPoll: (value) => (statusPoll = value)
		},
		{
			refreshAiConfig,
			refreshSnapshot,
			loadProviderStatus,
			normalizeBackendPreference
		}
	);
	createSettingsLifecycle(lifecyclePort);

	async function downloadLatexSupport() {
		latexError = '';
		latexDownloading = true;
		try {
			await invoke('prewarm_tectonic');
		} catch (e) {
			// The backend also emits a `latex://download` error event, but guard
			// against the invoke itself rejecting (e.g. no network at all).
			latexError = String(e);
			latexDownloading = false;
		}
	}

	async function downloadBackend(backend: string) {
		try {
			await invoke('download_llama_backend', { backend });
		} catch (e) {
			console.error('Backend download failed:', e);
		}
	}

	async function selectInferenceEngine(engine: 'llama_cpp' | 'beellama') {
		if (engine === 'beellama' && !installedBeeBackends.includes(recommendedBeeBackend)) {
			beeDownloadActive = true;
			try {
				await invoke('download_bee_backend', { backend: recommendedBeeBackend });
				await invoke('set_inference_engine', { engine: 'beellama' });
				// Force the normal launcher/health/fallback path now so the
				// selector reports a broken Bee build immediately.
				await invoke('warm_llama_server');
				await loadProviderStatus();
			} catch (e) {
				beeDownloadActive = false;
				console.error('BeeLlama installation failed:', e);
			}
			return;
		}
		await invoke('set_inference_engine', { engine });
		inferenceEngine = engine;
		await loadProviderStatus();
	}

	async function changeWorkspace() {
		const picked = await open({
			directory: true,
			multiple: false,
			title: 'Choose your markdown workspace'
		});
		if (typeof picked === 'string') {
			await invoke('set_workspace', { workspacePath: picked });
			await refreshSnapshot();
		}
	}

	async function rebuildIndex() {
		isRebuilding = true;
		try {
			const snapshot = await invoke<AppSnapshot>('rebuild_index');
			indexState = snapshot.indexState ?? null;
		} finally {
			isRebuilding = false;
		}
	}

	async function selectModel() {
		try {
			const selected = await open({
				multiple: false,
				filters: [
					{
						name: 'GGUF Model',
						extensions: ['gguf']
					}
				]
			});

			if (selected && !Array.isArray(selected)) {
				currentModelPath = selected;
				await saveModelPath();
			}
		} catch (error) {
			console.error('Failed to open file dialog:', error);
		}
	}

	function dispose() {
		if (statusPoll) clearInterval(statusPoll);
		persistence.dispose();
	}

	return createBoundController(
		() => ({
			...lifecyclePort,
			...shortcutsPort,
			...persistencePort,
			...aiConfigControllerPort,
			...retrievalControllerPort,
			backendLabel,
			formatMB,
			gpuIssue,
			inferenceEngine,
			installedBackends,
			installedBeeBackends,
			isRebuilding,
			providerDetail,
			recommendedBeeBackend,
			computeStatus: (() => {
				return computeStatus;
			})(),

			activeProvider,
			activeWorkspacePath,
			backgroundError,
			gpuAvailable,
			gpus,
			indexState,
			nvidiaDetected,
			providerHealthy,

			...lifecyclePort
		}),
		{
			currentModelPath: (value) => (currentModelPath = value),
			contextSize: (value) => (contextSize = value),
			gpuLayers: (value) => (gpuLayers = value),
			threads: (value) => (threads = value),
			recommendedThreads: (value) => (recommendedThreads = value),
			temperature: (value) => (temperature = value),
			topP: (value) => (topP = value),
			maxTurns: (value) => (maxTurns = value),
			thinking: (value) => (thinking = value),
			autoOffload: (value) => (autoOffload = value),
			deterministicTools: (value) => (deterministicTools = value),
			toolGating: (value) => (toolGating = value),
			promptCache: (value) => (promptCache = value),
			llamaCache: (value) => (llamaCache = value),
			extraArgs: (value) => (extraArgs = value),
			activeWorkspacePath: (value) => (activeWorkspacePath = value),
			indexState: (value) => (indexState = value),
			activeProvider: (value) => (activeProvider = value),
			inferenceEngine: (value) => (inferenceEngine = value),
			activeEngine: (value) => (activeEngine = value),
			installedBeeBackends: (value) => (installedBeeBackends = value),
			downloadableBeeBackends: (value) => (downloadableBeeBackends = value),
			beeDownloadActive: (value) => (beeDownloadActive = value),
			backendPreference: (value) => (backendPreference = value),
			downloadableBackends: (value) => (downloadableBackends = value),
			download: (value) => (download = value),
			activeBackend: (value) => (activeBackend = value),
			nvidiaDetected: (value) => (nvidiaDetected = value),
			gpuAvailable: (value) => (gpuAvailable = value),
			gpus: (value) => (gpus = value),
			installedBackends: (value) => (installedBackends = value),
			backendFellBack: (value) => (backendFellBack = value),
			providerHealthy: (value) => (providerHealthy = value),
			providerDetail: (value) => (providerDetail = value),
			latexCache: (value) => (latexCache = value),
			latexDownloading: (value) => (latexDownloading = value),
			latexDownloadBytes: (value) => (latexDownloadBytes = value),
			latexError: (value) => (latexError = value),
			quickShortcut: (value) => (quickShortcut = value),
			quickRecording: (value) => (quickRecording = value),
			quickShortcutError: (value) => (quickShortcutError = value),
			chatShortcutRecording: (value) => (chatShortcutRecording = value),
			chatShortcutError: (value) => (chatShortcutError = value),
			startWithSystem: (value) => (startWithSystem = value),
			backgroundError: (value) => (backgroundError = value),
			statusPoll: (value) => (statusPoll = value),
			isSaving: (value) => (isSaving = value),
			isRebuilding: (value) => (isRebuilding = value),
			saved: (value) => (saved = value),
			enableJupyterExecution: (value) => (enableJupyterExecution = value),
			ohPort: (value) => (ohPort = value),
			ohBinPath: (value) => (ohBinPath = value),
			ohToolMode: (value) => (ohToolMode = value),
			ohStrict: (value) => (ohStrict = value),
			ohPromptTools: (value) => (ohPromptTools = value),
			ohCallOnly: (value) => (ohCallOnly = value),
			ohNoThink: (value) => (ohNoThink = value),
			ohToolChoice: (value) => (ohToolChoice = value),
			ohTemplateKwargs: (value) => (ohTemplateKwargs = value),
			ohMaxCalls: (value) => (ohMaxCalls = value),
			ohTotalMax: (value) => (ohTotalMax = value),
			ohToolTimeout: (value) => (ohToolTimeout = value),
			ohBaseUrl: (value) => (ohBaseUrl = value),
			externalEnabled: (value) => (externalEnabled = value),
			externalBaseUrl: (value) => (externalBaseUrl = value),
			externalModel: (value) => (externalModel = value),
			externalApiKey: (value) => (externalApiKey = value),
			ohSaving: (value) => (ohSaving = value),
			aiConfig: (value) => (aiConfig = value),
			aiConfigBusy: (value) => (aiConfigBusy = value),
			aiConfigMessage: (value) => (aiConfigMessage = value),
			showAiConfig: (value) => (showAiConfig = value),
			aiConfigText: (value) => (aiConfigText = value),
			aiConfigSearch: (value) => (aiConfigSearch = value),
			aiConfigSearchInput: (value) => (aiConfigSearchInput = value),
			aiConfigEditor: (value) => (aiConfigEditor = value),
			aiConfigSearchIndex: (value) => (aiConfigSearchIndex = value),
			searxngUrl: (value) => (searxngUrl = value),
			embedModelPath: (value) => (embedModelPath = value),
			rerankerModelPath: (value) => (rerankerModelPath = value),
			modelDownloadError: (value) => (modelDownloadError = value),
			ocrStatus: (value) => (ocrStatus = value),
			downloadingModel: (value) => (downloadingModel = value),
			modelProfiles: (value) => {
				modelProfiles = value;
			}
		},
		{
			applyShortcut,
			startRecording,
			startChatShortcutRecording,
			selectBackend,
			gotoConfigMatch,
			handleConfigSearchKeydown,
			handleAiConfigKeydown,
			configMatchCount,
			refreshAiConfig,
			validateAiConfig,
			applyAiConfig,
			copyAiConfigPath,
			openAiConfig,
			saveAiConfigText,
			saveSearxng,
			pickEmbedModel,
			clearEmbedModel,
			pickRerankerModel,
			clearRerankerModel,
			downloadBuiltInModel,
			saveOcrSettings,
			refreshSnapshot,
			loadProviderStatus,
			goBack,
			downloadLatexSupport,
			downloadBackend,
			selectInferenceEngine,
			changeWorkspace,
			rebuildIndex,
			selectModel,
			saveModelPath,
			saveAdvancedConfig,
			debounceSave,
			addExtraArg,
			removeExtraArg,
			toggleJupyterExecution,
			pickOpenharnBin,
			changeToolMode,
			saveOpenharn,
			dispose
		}
	);
}

export type SettingsController = ReturnType<typeof createSettingsController>;
