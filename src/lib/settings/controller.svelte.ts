import { onDestroy } from 'svelte';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { open } from '@tauri-apps/plugin-dialog';
import { goto } from '$app/navigation';
import type { AppSnapshot, IndexState, ProviderStatus } from '$lib/types';
import { backendPreference as normalizeBackendPreference, configMatchPositions as findConfigMatches, inferencePayload, openharnPayload, recommendedBeeBackend as chooseBeeBackend } from '$lib/settings/model';
import type { OpenharnForm } from '$lib/settings/types';
import { createSettingsLifecycle } from './lifecycle.svelte';
import { createSettingsShortcuts } from './shortcuts.svelte';

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
		get quickShortcut() { return quickShortcut; },
		get quickRecording() { return quickRecording; }, set quickRecording(value) { quickRecording = value; },
		get quickShortcutError() { return quickShortcutError; }, set quickShortcutError(value) { quickShortcutError = value; },
		get chatShortcutRecording() { return chatShortcutRecording; }, set chatShortcutRecording(value) { chatShortcutRecording = value; },
		get chatShortcutError() { return chatShortcutError; }, set chatShortcutError(value) { chatShortcutError = value; }
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

	// openharn sidecar settings (Settings > Agent).
	type OpenharnSettings = {
		port: number | null;
		bin_path: string | null;
		tool_mode: 'auto' | 'native' | 'prompt';
		strict: boolean;
		prompt_tools: boolean;
		call_only: boolean;
		no_think: boolean;
		tool_choice: string | null;
		template_kwargs: string | null;
		narrow: boolean;
		slm: boolean;
		max_calls: number | null;
		total_max: number | null;
		tool_timeout_secs: number | null;
		tool_subset: string | null;
		base_url: string | null;
		external_enabled: boolean;
		external_base_url: string | null;
		external_model: string | null;
		external_api_key: string | null;
	};
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

	type AiConfigStatus = {
		configPath: string;
		schemaPath: string;
		candidateHash: string | null;
		appliedHash: string | null;
		hasUnappliedChanges: boolean;
		validationState: string;
		activeProfile: string | null;
		runtimeId: string | null;
		modelPath: string | null;
		aiAvailable: boolean;
		errors: { path: string; category: string; message: string }[];
	};
	let aiConfig = $state<AiConfigStatus | null>(null);
	let aiConfigBusy = $state(false);
	let aiConfigMessage = $state('');
	let showAiConfig = $state(false);
	let aiConfigText = $state('');
	let aiConfigSearch = $state('');
	let aiConfigSearchInput: HTMLInputElement;
	let aiConfigEditor: HTMLTextAreaElement;
	let aiConfigSearchIndex = $state(-1);
	function configMatchPositions() {
		return findConfigMatches(aiConfigText, aiConfigSearch);
	}
	function gotoConfigMatch(direction = 1) {
		const positions = configMatchPositions();
		if (!positions.length) return;
		aiConfigSearchIndex = (aiConfigSearchIndex + direction + positions.length) % positions.length;
		const start = positions[aiConfigSearchIndex];
		const end = start + aiConfigSearch.trim().length;
		aiConfigEditor?.focus();
		aiConfigEditor?.setSelectionRange(start, end);
		if (aiConfigEditor) {
			const lineHeight = parseFloat(getComputedStyle(aiConfigEditor).lineHeight) || 20;
			const line = aiConfigText.slice(0, start).split('\n').length - 1;
			aiConfigEditor.scrollTop = Math.max(0, line * lineHeight - aiConfigEditor.clientHeight / 2);
		}
	}
	function handleConfigSearchKeydown(event: KeyboardEvent) {
		if (event.key === 'Enter') { event.preventDefault(); gotoConfigMatch(event.shiftKey ? -1 : 1); }
	}
	function handleAiConfigKeydown(event: KeyboardEvent) {
		if (!showAiConfig || !(event.ctrlKey || event.metaKey) || event.key.toLowerCase() !== 'f') return;
		event.preventDefault();
		event.stopPropagation();
		aiConfigSearchInput?.focus();
		aiConfigSearchInput?.select();
	}
	function configMatchCount() {
		return configMatchPositions().length;
	}
	async function refreshAiConfig() {
		aiConfig = await invoke<AiConfigStatus>('get_ai_config_status');
	}
	async function validateAiConfig() {
		aiConfigBusy = true; aiConfigMessage = '';
		try { aiConfig = await invoke<AiConfigStatus>('validate_ai_config'); aiConfigMessage = 'Configuration validated.'; }
		catch (e) { aiConfigMessage = String(e); await refreshAiConfig(); }
		finally { aiConfigBusy = false; }
	}
	async function applyAiConfig() {
		if (!aiConfig?.candidateHash) return;
		aiConfigBusy = true; aiConfigMessage = '';
		try { aiConfig = await invoke<AiConfigStatus>('apply_ai_config', { candidateHash: aiConfig.candidateHash }); aiConfigMessage = 'Configuration applied.'; }
		catch (e) { aiConfigMessage = String(e); }
		finally { aiConfigBusy = false; }
	}
	async function copyAiConfigPath() {
		if (aiConfig?.configPath) await navigator.clipboard?.writeText(aiConfig.configPath);
	}
	async function openAiConfig() {
		try { await invoke('open_ai_config_file'); aiConfigText = await invoke<string>('read_ai_config'); showAiConfig = true; }
		catch (e) { aiConfigMessage = String(e); }
	}
	async function saveAiConfigText() {
		aiConfigBusy = true;
		try { await invoke('save_ai_config', { contents: aiConfigText }); await refreshAiConfig(); showAiConfig = false; aiConfigMessage = 'Configuration saved. Validate it before applying.'; }
		catch (e) { aiConfigMessage = String(e); }
		finally { aiConfigBusy = false; }
	}

	// Web search + embeddings/RAG + model compatibility (Phase 5).
	let searxngUrl = $state('');
	let embedModelPath = $state('');
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

	async function saveSearxng() {
		await invoke('set_searxng_url', { url: searxngUrl.trim() || null });
	}
	async function pickEmbedModel() {
		const picked = await open({
			multiple: false,
			filters: [{ name: 'GGUF model', extensions: ['gguf'] }]
		});
		if (typeof picked === 'string') {
			embedModelPath = picked;
			await invoke('set_embed_model_path', { path: picked });
		}
	}
	async function clearEmbedModel() {
		embedModelPath = '';
		await invoke('set_embed_model_path', { path: null });
	}

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
			goto('/');
		}
	}

	createSettingsLifecycle({
		refreshAiConfig,
		refreshSnapshot,
		loadProviderStatus,
		normalizeBackendPreference,
		get downloadableBackends() { return downloadableBackends; }, set downloadableBackends(value) { downloadableBackends = value; },
		get downloadableBeeBackends() { return downloadableBeeBackends; }, set downloadableBeeBackends(value) { downloadableBeeBackends = value; },
		get backendPreference() { return backendPreference; }, set backendPreference(value) { backendPreference = value; },
		get thinking() { return thinking; }, set thinking(value) { thinking = value; },
		get autoOffload() { return autoOffload; }, set autoOffload(value) { autoOffload = value; },
		get deterministicTools() { return deterministicTools; }, set deterministicTools(value) { deterministicTools = value; },
		get toolGating() { return toolGating; }, set toolGating(value) { toolGating = value; },
		get promptCache() { return promptCache; }, set promptCache(value) { promptCache = value; },
		get recommendedThreads() { return recommendedThreads; }, set recommendedThreads(value) { recommendedThreads = value; },
		get llamaCache() { return llamaCache; }, set llamaCache(value) { llamaCache = value; },
		get searxngUrl() { return searxngUrl; }, set searxngUrl(value) { searxngUrl = value; },
		get embedModelPath() { return embedModelPath; }, set embedModelPath(value) { embedModelPath = value; },
		get quickShortcut() { return quickShortcut; }, set quickShortcut(value) { quickShortcut = value; },
		get startWithSystem() { return startWithSystem; }, set startWithSystem(value) { startWithSystem = value; },
		get ohPort() { return ohPort; }, set ohPort(value) { ohPort = value; },
		get ohBinPath() { return ohBinPath; }, set ohBinPath(value) { ohBinPath = value; },
		get ohToolMode() { return ohToolMode; }, set ohToolMode(value) { ohToolMode = value; },
		get ohStrict() { return ohStrict; }, set ohStrict(value) { ohStrict = value; },
		get ohPromptTools() { return ohPromptTools; }, set ohPromptTools(value) { ohPromptTools = value; },
		get ohCallOnly() { return ohCallOnly; }, set ohCallOnly(value) { ohCallOnly = value; },
		get ohNoThink() { return ohNoThink; }, set ohNoThink(value) { ohNoThink = value; },
		get ohToolChoice() { return ohToolChoice; }, set ohToolChoice(value) { ohToolChoice = value; },
		get ohTemplateKwargs() { return ohTemplateKwargs; }, set ohTemplateKwargs(value) { ohTemplateKwargs = value; },
		get ohMaxCalls() { return ohMaxCalls; }, set ohMaxCalls(value) { ohMaxCalls = value; },
		get ohTotalMax() { return ohTotalMax; }, set ohTotalMax(value) { ohTotalMax = value; },
		get ohToolTimeout() { return ohToolTimeout; }, set ohToolTimeout(value) { ohToolTimeout = value; },
		get ohBaseUrl() { return ohBaseUrl; }, set ohBaseUrl(value) { ohBaseUrl = value; },
		get externalEnabled() { return externalEnabled; }, set externalEnabled(value) { externalEnabled = value; },
		get externalBaseUrl() { return externalBaseUrl; }, set externalBaseUrl(value) { externalBaseUrl = value; },
		get externalModel() { return externalModel; }, set externalModel(value) { externalModel = value; },
		get externalApiKey() { return externalApiKey; }, set externalApiKey(value) { externalApiKey = value; },
		get modelProfiles() { return modelProfiles; }, set modelProfiles(value) { modelProfiles = value; },
		get currentModelPath() { return currentModelPath; }, set currentModelPath(value) { currentModelPath = value; },
		get contextSize() { return contextSize; }, set contextSize(value) { contextSize = value; },
		get gpuLayers() { return gpuLayers; }, set gpuLayers(value) { gpuLayers = value; },
		get threads() { return threads; }, set threads(value) { threads = value; },
		get temperature() { return temperature; }, set temperature(value) { temperature = value; },
		get topP() { return topP; }, set topP(value) { topP = value; },
		get maxTurns() { return maxTurns; }, set maxTurns(value) { maxTurns = value; },
		get extraArgs() { return extraArgs; }, set extraArgs(value) { extraArgs = value; },
		get enableJupyterExecution() { return enableJupyterExecution; }, set enableJupyterExecution(value) { enableJupyterExecution = value; },
		get latexCache() { return latexCache; }, set latexCache(value) { latexCache = value; },
		get latexDownloading() { return latexDownloading; }, set latexDownloading(value) { latexDownloading = value; },
		get latexDownloadBytes() { return latexDownloadBytes; }, set latexDownloadBytes(value) { latexDownloadBytes = value; },
		get latexError() { return latexError; }, set latexError(value) { latexError = value; },
		get activeBackend() { return activeBackend; }, set activeBackend(value) { activeBackend = value; },
		get activeEngine() { return activeEngine; }, set activeEngine(value) { activeEngine = value; },
		get backendFellBack() { return backendFellBack; }, set backendFellBack(value) { backendFellBack = value; },
		get beeDownloadActive() { return beeDownloadActive; }, set beeDownloadActive(value) { beeDownloadActive = value; },
		get download() { return download; }, set download(value) { download = value; },
		get statusPoll() { return statusPoll; }, set statusPoll(value) { statusPoll = value; },
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

	async function saveModelPath() {
		if (!currentModelPath) return;

		isSaving = true;
		saved = false;
		try {
			await invoke('set_llama_model_path', { modelPath: currentModelPath });
			saved = true;
			setTimeout(() => {
				saved = false;
			}, 3000);
		} catch (error) {
			console.error('Failed to save model path:', error);
			alert('Failed to save model path: ' + error);
		} finally {
			isSaving = false;
		}
	}

	async function saveAdvancedConfig() {
		isSaving = true;
		saved = false;
		try {
			await invoke('set_llama_advanced_config', inferencePayload({
				contextSize,
				gpuLayers,
				threads,
				temperature,
				topP,
				maxTurns,
				extraArgs,
				backendPreference,
				thinking,
				autoOffload
			}));
			saved = true;
			setTimeout(() => {
				saved = false;
			}, 3000);
		} catch (error) {
			console.error('Failed to save advanced config:', error);
			alert('Failed to save advanced config: ' + error);
		} finally {
			isSaving = false;
		}
	}

	let saveTimeout: ReturnType<typeof setTimeout>;
	function debounceSave() {
		clearTimeout(saveTimeout);
		saveTimeout = setTimeout(saveAdvancedConfig, 500);
	}

	function addExtraArg() {
		extraArgs.push('');
		debounceSave();
	}

	function removeExtraArg(index: number) {
		extraArgs.splice(index, 1);
		debounceSave();
	}

	function toggleJupyterExecution() {
		enableJupyterExecution = !enableJupyterExecution;
		localStorage.setItem('myelin_jupyter_exec', enableJupyterExecution.toString());
	}

	async function pickOpenharnBin() {
		const picked = await open({ multiple: false, title: 'Choose openharn-myelin binary' });
		if (typeof picked === 'string') {
			ohBinPath = picked;
			await saveOpenharn();
		}
	}

	function changeToolMode() {
		if (ohToolMode !== 'prompt') {
			ohStrict = false;
			ohCallOnly = false;
			ohPromptTools = false;
		}
		void saveOpenharn();
	}

	async function saveOpenharn() {
		ohSaving = true;
		try {
			const settings: OpenharnForm = {
				port: ohPort,
				binPath: ohBinPath,
				toolMode: ohToolMode,
				strict: ohStrict,
				promptTools: ohPromptTools,
				callOnly: ohCallOnly,
				noThink: ohNoThink,
				toolChoice: ohToolChoice,
				templateKwargs: ohTemplateKwargs,
				maxCalls: ohMaxCalls,
				totalMax: ohTotalMax,
				toolTimeoutSecs: ohToolTimeout,
				baseUrl: ohBaseUrl,
				externalEnabled,
				externalBaseUrl,
				externalModel,
				externalApiKey
			};
			await invoke('set_openharn_settings', { settings: openharnPayload(settings) });
			saved = true;
			setTimeout(() => {
				saved = false;
			}, 3000);
		} catch (error) {
			console.error('Failed to save agent settings:', error);
			alert('Failed to save agent settings: ' + error);
		} finally {
			ohSaving = false;
		}
	}

	function dispose() {
		if (statusPoll) clearInterval(statusPoll);
		if (saveTimeout) clearTimeout(saveTimeout);
	}

	return {
		get currentModelPath() { return currentModelPath; }, set currentModelPath(value) { currentModelPath = value; },
		get contextSize() { return contextSize; }, set contextSize(value) { contextSize = value; },
		get gpuLayers() { return gpuLayers; }, set gpuLayers(value) { gpuLayers = value; },
		get threads() { return threads; }, set threads(value) { threads = value; },
		get recommendedThreads() { return recommendedThreads; }, set recommendedThreads(value) { recommendedThreads = value; },
		get temperature() { return temperature; }, set temperature(value) { temperature = value; },
		get topP() { return topP; }, set topP(value) { topP = value; },
		get maxTurns() { return maxTurns; }, set maxTurns(value) { maxTurns = value; },
		get thinking() { return thinking; }, set thinking(value) { thinking = value; },
		get autoOffload() { return autoOffload; }, set autoOffload(value) { autoOffload = value; },
		get deterministicTools() { return deterministicTools; }, set deterministicTools(value) { deterministicTools = value; },
		get toolGating() { return toolGating; }, set toolGating(value) { toolGating = value; },
		get promptCache() { return promptCache; }, set promptCache(value) { promptCache = value; },
		get llamaCache() { return llamaCache; }, set llamaCache(value) { llamaCache = value; },
		get extraArgs() { return extraArgs; }, set extraArgs(value) { extraArgs = value; },
		get activeWorkspacePath() { return activeWorkspacePath; }, set activeWorkspacePath(value) { activeWorkspacePath = value; },
		get indexState() { return indexState; }, set indexState(value) { indexState = value; },
		get activeProvider() { return activeProvider; }, set activeProvider(value) { activeProvider = value; },
		get inferenceEngine() { return inferenceEngine; }, set inferenceEngine(value) { inferenceEngine = value; },
		get activeEngine() { return activeEngine; }, set activeEngine(value) { activeEngine = value; },
		get installedBeeBackends() { return installedBeeBackends; }, set installedBeeBackends(value) { installedBeeBackends = value; },
		get downloadableBeeBackends() { return downloadableBeeBackends; }, set downloadableBeeBackends(value) { downloadableBeeBackends = value; },
		get beeDownloadActive() { return beeDownloadActive; }, set beeDownloadActive(value) { beeDownloadActive = value; },
		get backendPreference() { return backendPreference; }, set backendPreference(value) { backendPreference = value; },
		get downloadableBackends() { return downloadableBackends; }, set downloadableBackends(value) { downloadableBackends = value; },
		get download() { return download; }, set download(value) { download = value; },
		get activeBackend() { return activeBackend; }, set activeBackend(value) { activeBackend = value; },
		get nvidiaDetected() { return nvidiaDetected; }, set nvidiaDetected(value) { nvidiaDetected = value; },
		get gpuAvailable() { return gpuAvailable; }, set gpuAvailable(value) { gpuAvailable = value; },
		get gpus() { return gpus; }, set gpus(value) { gpus = value; },
		get installedBackends() { return installedBackends; }, set installedBackends(value) { installedBackends = value; },
		get backendFellBack() { return backendFellBack; }, set backendFellBack(value) { backendFellBack = value; },
		get providerHealthy() { return providerHealthy; }, set providerHealthy(value) { providerHealthy = value; },
		get providerDetail() { return providerDetail; }, set providerDetail(value) { providerDetail = value; },
		get latexCache() { return latexCache; }, set latexCache(value) { latexCache = value; },
		get latexDownloading() { return latexDownloading; }, set latexDownloading(value) { latexDownloading = value; },
		get latexDownloadBytes() { return latexDownloadBytes; }, set latexDownloadBytes(value) { latexDownloadBytes = value; },
		get latexError() { return latexError; }, set latexError(value) { latexError = value; },
		get quickShortcut() { return quickShortcut; }, set quickShortcut(value) { quickShortcut = value; },
		get quickRecording() { return quickRecording; }, set quickRecording(value) { quickRecording = value; },
		get quickShortcutError() { return quickShortcutError; }, set quickShortcutError(value) { quickShortcutError = value; },
		get chatShortcutRecording() { return chatShortcutRecording; }, set chatShortcutRecording(value) { chatShortcutRecording = value; },
		get chatShortcutError() { return chatShortcutError; }, set chatShortcutError(value) { chatShortcutError = value; },
		get startWithSystem() { return startWithSystem; }, set startWithSystem(value) { startWithSystem = value; },
		get backgroundError() { return backgroundError; }, set backgroundError(value) { backgroundError = value; },
		get statusPoll() { return statusPoll; }, set statusPoll(value) { statusPoll = value; },
		get isSaving() { return isSaving; }, set isSaving(value) { isSaving = value; },
		get isRebuilding() { return isRebuilding; }, set isRebuilding(value) { isRebuilding = value; },
		get saved() { return saved; }, set saved(value) { saved = value; },
		get enableJupyterExecution() { return enableJupyterExecution; }, set enableJupyterExecution(value) { enableJupyterExecution = value; },
		get ohPort() { return ohPort; }, set ohPort(value) { ohPort = value; },
		get ohBinPath() { return ohBinPath; }, set ohBinPath(value) { ohBinPath = value; },
		get ohToolMode() { return ohToolMode; }, set ohToolMode(value) { ohToolMode = value; },
		get ohStrict() { return ohStrict; }, set ohStrict(value) { ohStrict = value; },
		get ohPromptTools() { return ohPromptTools; }, set ohPromptTools(value) { ohPromptTools = value; },
		get ohCallOnly() { return ohCallOnly; }, set ohCallOnly(value) { ohCallOnly = value; },
		get ohNoThink() { return ohNoThink; }, set ohNoThink(value) { ohNoThink = value; },
		get ohToolChoice() { return ohToolChoice; }, set ohToolChoice(value) { ohToolChoice = value; },
		get ohTemplateKwargs() { return ohTemplateKwargs; }, set ohTemplateKwargs(value) { ohTemplateKwargs = value; },
		get ohMaxCalls() { return ohMaxCalls; }, set ohMaxCalls(value) { ohMaxCalls = value; },
		get ohTotalMax() { return ohTotalMax; }, set ohTotalMax(value) { ohTotalMax = value; },
		get ohToolTimeout() { return ohToolTimeout; }, set ohToolTimeout(value) { ohToolTimeout = value; },
		get ohBaseUrl() { return ohBaseUrl; }, set ohBaseUrl(value) { ohBaseUrl = value; },
		get externalEnabled() { return externalEnabled; }, set externalEnabled(value) { externalEnabled = value; },
		get externalBaseUrl() { return externalBaseUrl; }, set externalBaseUrl(value) { externalBaseUrl = value; },
		get externalModel() { return externalModel; }, set externalModel(value) { externalModel = value; },
		get externalApiKey() { return externalApiKey; }, set externalApiKey(value) { externalApiKey = value; },
		get ohSaving() { return ohSaving; }, set ohSaving(value) { ohSaving = value; },
		get aiConfig() { return aiConfig; }, set aiConfig(value) { aiConfig = value; },
		get aiConfigBusy() { return aiConfigBusy; }, set aiConfigBusy(value) { aiConfigBusy = value; },
		get aiConfigMessage() { return aiConfigMessage; }, set aiConfigMessage(value) { aiConfigMessage = value; },
		get showAiConfig() { return showAiConfig; }, set showAiConfig(value) { showAiConfig = value; },
		get aiConfigText() { return aiConfigText; }, set aiConfigText(value) { aiConfigText = value; },
		get aiConfigSearch() { return aiConfigSearch; }, set aiConfigSearch(value) { aiConfigSearch = value; },
		get aiConfigSearchInput() { return aiConfigSearchInput; }, set aiConfigSearchInput(value) { aiConfigSearchInput = value; },
		get aiConfigEditor() { return aiConfigEditor; }, set aiConfigEditor(value) { aiConfigEditor = value; },
		get aiConfigSearchIndex() { return aiConfigSearchIndex; }, set aiConfigSearchIndex(value) { aiConfigSearchIndex = value; },
		get searxngUrl() { return searxngUrl; }, set searxngUrl(value) { searxngUrl = value; },
		get embedModelPath() { return embedModelPath; }, set embedModelPath(value) { embedModelPath = value; },
		get modelProfiles() { return modelProfiles; }, set modelProfiles(value) { modelProfiles = value; },
		get saveTimeout() { return saveTimeout; }, set saveTimeout(value) { saveTimeout = value; },
		get formatMB() { return formatMB; },
		get hasGpuBuild() { return hasGpuBuild; },
		get backendLabel() { return backendLabel; },
		get recommendedBeeBackend() { return recommendedBeeBackend; },
		get gpuIssue() { return gpuIssue; },
		get computeStatus() { return computeStatus; },
		applyShortcut,
		startRecording,
		startChatShortcutRecording,
		selectBackend,
		configMatchPositions,
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
	};
}

export type SettingsController = ReturnType<typeof createSettingsController>;
