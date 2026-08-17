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
	const shortcuts = createSettingsShortcuts({
		get quickShortcut() {
			return quickShortcut;
		},
		get quickRecording() {
			return quickRecording;
		},
		set quickRecording(value) {
			quickRecording = value;
		},
		get quickShortcutError() {
			return quickShortcutError;
		},
		set quickShortcutError(value) {
			quickShortcutError = value;
		},
		get chatShortcutRecording() {
			return chatShortcutRecording;
		},
		set chatShortcutRecording(value) {
			chatShortcutRecording = value;
		},
		get chatShortcutError() {
			return chatShortcutError;
		},
		set chatShortcutError(value) {
			chatShortcutError = value;
		}
	});
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
	const persistence = createSettingsPersistence({
		get currentModelPath() {
			return currentModelPath;
		},
		get contextSize() {
			return contextSize;
		},
		get gpuLayers() {
			return gpuLayers;
		},
		get threads() {
			return threads;
		},
		get temperature() {
			return temperature;
		},
		get topP() {
			return topP;
		},
		get maxTurns() {
			return maxTurns;
		},
		get extraArgs() {
			return extraArgs;
		},
		set extraArgs(value) {
			extraArgs = value;
		},
		get backendPreference() {
			return backendPreference;
		},
		get thinking() {
			return thinking;
		},
		get autoOffload() {
			return autoOffload;
		},
		get ohPort() {
			return ohPort;
		},
		get ohBinPath() {
			return ohBinPath;
		},
		set ohBinPath(value) {
			ohBinPath = value;
		},
		get ohToolMode() {
			return ohToolMode;
		},
		get ohStrict() {
			return ohStrict;
		},
		set ohStrict(value) {
			ohStrict = value;
		},
		get ohPromptTools() {
			return ohPromptTools;
		},
		set ohPromptTools(value) {
			ohPromptTools = value;
		},
		get ohCallOnly() {
			return ohCallOnly;
		},
		set ohCallOnly(value) {
			ohCallOnly = value;
		},
		get ohNoThink() {
			return ohNoThink;
		},
		get ohToolChoice() {
			return ohToolChoice;
		},
		get ohTemplateKwargs() {
			return ohTemplateKwargs;
		},
		get ohMaxCalls() {
			return ohMaxCalls;
		},
		get ohTotalMax() {
			return ohTotalMax;
		},
		get ohToolTimeout() {
			return ohToolTimeout;
		},
		get ohBaseUrl() {
			return ohBaseUrl;
		},
		get externalEnabled() {
			return externalEnabled;
		},
		get externalBaseUrl() {
			return externalBaseUrl;
		},
		get externalModel() {
			return externalModel;
		},
		get externalApiKey() {
			return externalApiKey;
		},
		set isSaving(value: boolean) {
			isSaving = value;
		},
		set saved(value: boolean) {
			saved = value;
		},
		set ohSaving(value: boolean) {
			ohSaving = value;
		},
		get enableJupyterExecution() {
			return enableJupyterExecution;
		},
		set enableJupyterExecution(value) {
			enableJupyterExecution = value;
		}
	});
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
	const aiConfigController = createAiConfigController({
		get aiConfig() {
			return aiConfig;
		},
		set aiConfig(value) {
			aiConfig = value;
		},
		get aiConfigBusy() {
			return aiConfigBusy;
		},
		set aiConfigBusy(value) {
			aiConfigBusy = value;
		},
		get aiConfigMessage() {
			return aiConfigMessage;
		},
		set aiConfigMessage(value) {
			aiConfigMessage = value;
		},
		get showAiConfig() {
			return showAiConfig;
		},
		set showAiConfig(value) {
			showAiConfig = value;
		},
		get aiConfigText() {
			return aiConfigText;
		},
		set aiConfigText(value) {
			aiConfigText = value;
		},
		get aiConfigSearch() {
			return aiConfigSearch;
		},
		get aiConfigSearchInput() {
			return aiConfigSearchInput;
		},
		get aiConfigEditor() {
			return aiConfigEditor;
		},
		get aiConfigSearchIndex() {
			return aiConfigSearchIndex;
		},
		set aiConfigSearchIndex(value) {
			aiConfigSearchIndex = value;
		}
	});
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

	const retrievalController = createRetrievalController({
		get searxngUrl() {
			return searxngUrl;
		},
		get embedModelPath() {
			return embedModelPath;
		},
		set embedModelPath(value) {
			embedModelPath = value;
		},
		get rerankerModelPath() {
			return rerankerModelPath;
		},
		set rerankerModelPath(value) {
			rerankerModelPath = value;
		},
		set modelDownloadError(value: string) {
			modelDownloadError = value;
		},
		set downloadingModel(value: string) {
			downloadingModel = value;
		},
		set ocrSaving(value: boolean) {
			ocrSaving = value;
		},
		set ocrStatus(value: OcrStatus | null) {
			ocrStatus = value;
		}
	});
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

	createSettingsLifecycle({
		refreshAiConfig,
		refreshSnapshot,
		loadProviderStatus,
		normalizeBackendPreference,
		get downloadableBackends() {
			return downloadableBackends;
		},
		set downloadableBackends(value) {
			downloadableBackends = value;
		},
		get downloadableBeeBackends() {
			return downloadableBeeBackends;
		},
		set downloadableBeeBackends(value) {
			downloadableBeeBackends = value;
		},
		get backendPreference() {
			return backendPreference;
		},
		set backendPreference(value) {
			backendPreference = value;
		},
		get thinking() {
			return thinking;
		},
		set thinking(value) {
			thinking = value;
		},
		get autoOffload() {
			return autoOffload;
		},
		set autoOffload(value) {
			autoOffload = value;
		},
		get deterministicTools() {
			return deterministicTools;
		},
		set deterministicTools(value) {
			deterministicTools = value;
		},
		get toolGating() {
			return toolGating;
		},
		set toolGating(value) {
			toolGating = value;
		},
		get promptCache() {
			return promptCache;
		},
		set promptCache(value) {
			promptCache = value;
		},
		get recommendedThreads() {
			return recommendedThreads;
		},
		set recommendedThreads(value) {
			recommendedThreads = value;
		},
		get llamaCache() {
			return llamaCache;
		},
		set llamaCache(value) {
			llamaCache = value;
		},
		get searxngUrl() {
			return searxngUrl;
		},
		set searxngUrl(value) {
			searxngUrl = value;
		},
		get embedModelPath() {
			return embedModelPath;
		},
		set embedModelPath(value) {
			embedModelPath = value;
		},
		get rerankerModelPath() {
			return rerankerModelPath;
		},
		set rerankerModelPath(value) {
			rerankerModelPath = value;
		},
		get modelDownloadError() {
			return modelDownloadError;
		},
		set modelDownloadError(value) {
			modelDownloadError = value;
		},
		get ocrStatus() {
			return ocrStatus;
		},
		set ocrStatus(value) {
			ocrStatus = value;
		},
		get ocrSaving() {
			return ocrSaving;
		},
		set ocrSaving(value) {
			ocrSaving = value;
		},
		get downloadingModel() {
			return downloadingModel;
		},
		set downloadingModel(value) {
			downloadingModel = value;
		},
		get quickShortcut() {
			return quickShortcut;
		},
		set quickShortcut(value) {
			quickShortcut = value;
		},
		get startWithSystem() {
			return startWithSystem;
		},
		set startWithSystem(value) {
			startWithSystem = value;
		},
		get ohPort() {
			return ohPort;
		},
		set ohPort(value) {
			ohPort = value;
		},
		get ohBinPath() {
			return ohBinPath;
		},
		set ohBinPath(value) {
			ohBinPath = value;
		},
		get ohToolMode() {
			return ohToolMode;
		},
		set ohToolMode(value) {
			ohToolMode = value;
		},
		get ohStrict() {
			return ohStrict;
		},
		set ohStrict(value) {
			ohStrict = value;
		},
		get ohPromptTools() {
			return ohPromptTools;
		},
		set ohPromptTools(value) {
			ohPromptTools = value;
		},
		get ohCallOnly() {
			return ohCallOnly;
		},
		set ohCallOnly(value) {
			ohCallOnly = value;
		},
		get ohNoThink() {
			return ohNoThink;
		},
		set ohNoThink(value) {
			ohNoThink = value;
		},
		get ohToolChoice() {
			return ohToolChoice;
		},
		set ohToolChoice(value) {
			ohToolChoice = value;
		},
		get ohTemplateKwargs() {
			return ohTemplateKwargs;
		},
		set ohTemplateKwargs(value) {
			ohTemplateKwargs = value;
		},
		get ohMaxCalls() {
			return ohMaxCalls;
		},
		set ohMaxCalls(value) {
			ohMaxCalls = value;
		},
		get ohTotalMax() {
			return ohTotalMax;
		},
		set ohTotalMax(value) {
			ohTotalMax = value;
		},
		get ohToolTimeout() {
			return ohToolTimeout;
		},
		set ohToolTimeout(value) {
			ohToolTimeout = value;
		},
		get ohBaseUrl() {
			return ohBaseUrl;
		},
		set ohBaseUrl(value) {
			ohBaseUrl = value;
		},
		get externalEnabled() {
			return externalEnabled;
		},
		set externalEnabled(value) {
			externalEnabled = value;
		},
		get externalBaseUrl() {
			return externalBaseUrl;
		},
		set externalBaseUrl(value) {
			externalBaseUrl = value;
		},
		get externalModel() {
			return externalModel;
		},
		set externalModel(value) {
			externalModel = value;
		},
		get externalApiKey() {
			return externalApiKey;
		},
		set externalApiKey(value) {
			externalApiKey = value;
		},
		get modelProfiles() {
			return modelProfiles;
		},
		set modelProfiles(value) {
			modelProfiles = value;
		},
		get currentModelPath() {
			return currentModelPath;
		},
		set currentModelPath(value) {
			currentModelPath = value;
		},
		get contextSize() {
			return contextSize;
		},
		set contextSize(value) {
			contextSize = value;
		},
		get gpuLayers() {
			return gpuLayers;
		},
		set gpuLayers(value) {
			gpuLayers = value;
		},
		get threads() {
			return threads;
		},
		set threads(value) {
			threads = value;
		},
		get temperature() {
			return temperature;
		},
		set temperature(value) {
			temperature = value;
		},
		get topP() {
			return topP;
		},
		set topP(value) {
			topP = value;
		},
		get maxTurns() {
			return maxTurns;
		},
		set maxTurns(value) {
			maxTurns = value;
		},
		get extraArgs() {
			return extraArgs;
		},
		set extraArgs(value) {
			extraArgs = value;
		},
		get enableJupyterExecution() {
			return enableJupyterExecution;
		},
		set enableJupyterExecution(value) {
			enableJupyterExecution = value;
		},
		get latexCache() {
			return latexCache;
		},
		set latexCache(value) {
			latexCache = value;
		},
		get latexDownloading() {
			return latexDownloading;
		},
		set latexDownloading(value) {
			latexDownloading = value;
		},
		get latexDownloadBytes() {
			return latexDownloadBytes;
		},
		set latexDownloadBytes(value) {
			latexDownloadBytes = value;
		},
		get latexError() {
			return latexError;
		},
		set latexError(value) {
			latexError = value;
		},
		get activeBackend() {
			return activeBackend;
		},
		set activeBackend(value) {
			activeBackend = value;
		},
		get activeEngine() {
			return activeEngine;
		},
		set activeEngine(value) {
			activeEngine = value;
		},
		get backendFellBack() {
			return backendFellBack;
		},
		set backendFellBack(value) {
			backendFellBack = value;
		},
		get beeDownloadActive() {
			return beeDownloadActive;
		},
		set beeDownloadActive(value) {
			beeDownloadActive = value;
		},
		get download() {
			return download;
		},
		set download(value) {
			download = value;
		},
		get statusPoll() {
			return statusPoll;
		},
		set statusPoll(value) {
			statusPoll = value;
		}
	});

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
			currentModelPath: (() => {
				return currentModelPath;
			})(),
			contextSize: (() => {
				return contextSize;
			})(),
			gpuLayers: (() => {
				return gpuLayers;
			})(),
			threads: (() => {
				return threads;
			})(),
			recommendedThreads: (() => {
				return recommendedThreads;
			})(),
			temperature: (() => {
				return temperature;
			})(),
			topP: (() => {
				return topP;
			})(),
			maxTurns: (() => {
				return maxTurns;
			})(),
			thinking: (() => {
				return thinking;
			})(),
			autoOffload: (() => {
				return autoOffload;
			})(),
			deterministicTools: (() => {
				return deterministicTools;
			})(),
			toolGating: (() => {
				return toolGating;
			})(),
			promptCache: (() => {
				return promptCache;
			})(),
			llamaCache: (() => {
				return llamaCache;
			})(),
			extraArgs: (() => {
				return extraArgs;
			})(),
			activeWorkspacePath: (() => {
				return activeWorkspacePath;
			})(),
			indexState: (() => {
				return indexState;
			})(),
			activeProvider: (() => {
				return activeProvider;
			})(),
			inferenceEngine: (() => {
				return inferenceEngine;
			})(),
			activeEngine: (() => {
				return activeEngine;
			})(),
			installedBeeBackends: (() => {
				return installedBeeBackends;
			})(),
			downloadableBeeBackends: (() => {
				return downloadableBeeBackends;
			})(),
			beeDownloadActive: (() => {
				return beeDownloadActive;
			})(),
			backendPreference: (() => {
				return backendPreference;
			})(),
			downloadableBackends: (() => {
				return downloadableBackends;
			})(),
			download: (() => {
				return download;
			})(),
			activeBackend: (() => {
				return activeBackend;
			})(),
			nvidiaDetected: (() => {
				return nvidiaDetected;
			})(),
			gpuAvailable: (() => {
				return gpuAvailable;
			})(),
			gpus: (() => {
				return gpus;
			})(),
			installedBackends: (() => {
				return installedBackends;
			})(),
			backendFellBack: (() => {
				return backendFellBack;
			})(),
			providerHealthy: (() => {
				return providerHealthy;
			})(),
			providerDetail: (() => {
				return providerDetail;
			})(),
			latexCache: (() => {
				return latexCache;
			})(),
			latexDownloading: (() => {
				return latexDownloading;
			})(),
			latexDownloadBytes: (() => {
				return latexDownloadBytes;
			})(),
			latexError: (() => {
				return latexError;
			})(),
			quickShortcut: (() => {
				return quickShortcut;
			})(),
			quickRecording: (() => {
				return quickRecording;
			})(),
			quickShortcutError: (() => {
				return quickShortcutError;
			})(),
			chatShortcutRecording: (() => {
				return chatShortcutRecording;
			})(),
			chatShortcutError: (() => {
				return chatShortcutError;
			})(),
			startWithSystem: (() => {
				return startWithSystem;
			})(),
			backgroundError: (() => {
				return backgroundError;
			})(),
			statusPoll: (() => {
				return statusPoll;
			})(),
			isSaving: (() => {
				return isSaving;
			})(),
			isRebuilding: (() => {
				return isRebuilding;
			})(),
			saved: (() => {
				return saved;
			})(),
			enableJupyterExecution: (() => {
				return enableJupyterExecution;
			})(),
			ohPort: (() => {
				return ohPort;
			})(),
			ohBinPath: (() => {
				return ohBinPath;
			})(),
			ohToolMode: (() => {
				return ohToolMode;
			})(),
			ohStrict: (() => {
				return ohStrict;
			})(),
			ohPromptTools: (() => {
				return ohPromptTools;
			})(),
			ohCallOnly: (() => {
				return ohCallOnly;
			})(),
			ohNoThink: (() => {
				return ohNoThink;
			})(),
			ohToolChoice: (() => {
				return ohToolChoice;
			})(),
			ohTemplateKwargs: (() => {
				return ohTemplateKwargs;
			})(),
			ohMaxCalls: (() => {
				return ohMaxCalls;
			})(),
			ohTotalMax: (() => {
				return ohTotalMax;
			})(),
			ohToolTimeout: (() => {
				return ohToolTimeout;
			})(),
			ohBaseUrl: (() => {
				return ohBaseUrl;
			})(),
			externalEnabled: (() => {
				return externalEnabled;
			})(),
			externalBaseUrl: (() => {
				return externalBaseUrl;
			})(),
			externalModel: (() => {
				return externalModel;
			})(),
			externalApiKey: (() => {
				return externalApiKey;
			})(),
			ohSaving: (() => {
				return ohSaving;
			})(),
			aiConfig: (() => {
				return aiConfig;
			})(),
			aiConfigBusy: (() => {
				return aiConfigBusy;
			})(),
			aiConfigMessage: (() => {
				return aiConfigMessage;
			})(),
			showAiConfig: (() => {
				return showAiConfig;
			})(),
			aiConfigText: (() => {
				return aiConfigText;
			})(),
			aiConfigSearch: (() => {
				return aiConfigSearch;
			})(),
			aiConfigSearchInput: (() => {
				return aiConfigSearchInput;
			})(),
			aiConfigEditor: (() => {
				return aiConfigEditor;
			})(),
			aiConfigSearchIndex: (() => {
				return aiConfigSearchIndex;
			})(),
			searxngUrl: (() => {
				return searxngUrl;
			})(),
			embedModelPath: (() => {
				return embedModelPath;
			})(),
			rerankerModelPath: (() => {
				return rerankerModelPath;
			})(),
			modelDownloadError: (() => {
				return modelDownloadError;
			})(),
			ocrStatus: (() => {
				return ocrStatus;
			})(),
			downloadingModel: (() => {
				return downloadingModel;
			})(),
			modelProfiles: (() => {
				return modelProfiles;
			})(),
			formatMB: (() => {
				return formatMB;
			})(),
			hasGpuBuild: (() => {
				return hasGpuBuild;
			})(),
			backendLabel: (() => {
				return backendLabel;
			})(),
			recommendedBeeBackend: (() => {
				return recommendedBeeBackend;
			})(),
			gpuIssue: (() => {
				return gpuIssue;
			})(),
			computeStatus: (() => {
				return computeStatus;
			})()
		}),
		{
			currentModelPath: (value) => {
				currentModelPath = value;
			},
			contextSize: (value) => {
				contextSize = value;
			},
			gpuLayers: (value) => {
				gpuLayers = value;
			},
			threads: (value) => {
				threads = value;
			},
			recommendedThreads: (value) => {
				recommendedThreads = value;
			},
			temperature: (value) => {
				temperature = value;
			},
			topP: (value) => {
				topP = value;
			},
			maxTurns: (value) => {
				maxTurns = value;
			},
			thinking: (value) => {
				thinking = value;
			},
			autoOffload: (value) => {
				autoOffload = value;
			},
			deterministicTools: (value) => {
				deterministicTools = value;
			},
			toolGating: (value) => {
				toolGating = value;
			},
			promptCache: (value) => {
				promptCache = value;
			},
			llamaCache: (value) => {
				llamaCache = value;
			},
			extraArgs: (value) => {
				extraArgs = value;
			},
			activeWorkspacePath: (value) => {
				activeWorkspacePath = value;
			},
			indexState: (value) => {
				indexState = value;
			},
			activeProvider: (value) => {
				activeProvider = value;
			},
			inferenceEngine: (value) => {
				inferenceEngine = value;
			},
			activeEngine: (value) => {
				activeEngine = value;
			},
			installedBeeBackends: (value) => {
				installedBeeBackends = value;
			},
			downloadableBeeBackends: (value) => {
				downloadableBeeBackends = value;
			},
			beeDownloadActive: (value) => {
				beeDownloadActive = value;
			},
			backendPreference: (value) => {
				backendPreference = value;
			},
			downloadableBackends: (value) => {
				downloadableBackends = value;
			},
			download: (value) => {
				download = value;
			},
			activeBackend: (value) => {
				activeBackend = value;
			},
			nvidiaDetected: (value) => {
				nvidiaDetected = value;
			},
			gpuAvailable: (value) => {
				gpuAvailable = value;
			},
			gpus: (value) => {
				gpus = value;
			},
			installedBackends: (value) => {
				installedBackends = value;
			},
			backendFellBack: (value) => {
				backendFellBack = value;
			},
			providerHealthy: (value) => {
				providerHealthy = value;
			},
			providerDetail: (value) => {
				providerDetail = value;
			},
			latexCache: (value) => {
				latexCache = value;
			},
			latexDownloading: (value) => {
				latexDownloading = value;
			},
			latexDownloadBytes: (value) => {
				latexDownloadBytes = value;
			},
			latexError: (value) => {
				latexError = value;
			},
			quickShortcut: (value) => {
				quickShortcut = value;
			},
			quickRecording: (value) => {
				quickRecording = value;
			},
			quickShortcutError: (value) => {
				quickShortcutError = value;
			},
			chatShortcutRecording: (value) => {
				chatShortcutRecording = value;
			},
			chatShortcutError: (value) => {
				chatShortcutError = value;
			},
			startWithSystem: (value) => {
				startWithSystem = value;
			},
			backgroundError: (value) => {
				backgroundError = value;
			},
			statusPoll: (value) => {
				statusPoll = value;
			},
			isSaving: (value) => {
				isSaving = value;
			},
			isRebuilding: (value) => {
				isRebuilding = value;
			},
			saved: (value) => {
				saved = value;
			},
			enableJupyterExecution: (value) => {
				enableJupyterExecution = value;
			},
			ohPort: (value) => {
				ohPort = value;
			},
			ohBinPath: (value) => {
				ohBinPath = value;
			},
			ohToolMode: (value) => {
				ohToolMode = value;
			},
			ohStrict: (value) => {
				ohStrict = value;
			},
			ohPromptTools: (value) => {
				ohPromptTools = value;
			},
			ohCallOnly: (value) => {
				ohCallOnly = value;
			},
			ohNoThink: (value) => {
				ohNoThink = value;
			},
			ohToolChoice: (value) => {
				ohToolChoice = value;
			},
			ohTemplateKwargs: (value) => {
				ohTemplateKwargs = value;
			},
			ohMaxCalls: (value) => {
				ohMaxCalls = value;
			},
			ohTotalMax: (value) => {
				ohTotalMax = value;
			},
			ohToolTimeout: (value) => {
				ohToolTimeout = value;
			},
			ohBaseUrl: (value) => {
				ohBaseUrl = value;
			},
			externalEnabled: (value) => {
				externalEnabled = value;
			},
			externalBaseUrl: (value) => {
				externalBaseUrl = value;
			},
			externalModel: (value) => {
				externalModel = value;
			},
			externalApiKey: (value) => {
				externalApiKey = value;
			},
			ohSaving: (value) => {
				ohSaving = value;
			},
			aiConfig: (value) => {
				aiConfig = value;
			},
			aiConfigBusy: (value) => {
				aiConfigBusy = value;
			},
			aiConfigMessage: (value) => {
				aiConfigMessage = value;
			},
			showAiConfig: (value) => {
				showAiConfig = value;
			},
			aiConfigText: (value) => {
				aiConfigText = value;
			},
			aiConfigSearch: (value) => {
				aiConfigSearch = value;
			},
			aiConfigSearchInput: (value) => {
				aiConfigSearchInput = value;
			},
			aiConfigEditor: (value) => {
				aiConfigEditor = value;
			},
			aiConfigSearchIndex: (value) => {
				aiConfigSearchIndex = value;
			},
			searxngUrl: (value) => {
				searxngUrl = value;
			},
			embedModelPath: (value) => {
				embedModelPath = value;
			},
			rerankerModelPath: (value) => {
				rerankerModelPath = value;
			},
			modelDownloadError: (value) => {
				modelDownloadError = value;
			},
			ocrStatus: (value) => {
				ocrStatus = value;
			},
			downloadingModel: (value) => {
				downloadingModel = value;
			},
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
