<script lang="ts">
	import { onMount, onDestroy } from 'svelte';
	import { invoke } from '@tauri-apps/api/core';
	import { listen } from '@tauri-apps/api/event';
	import { open } from '@tauri-apps/plugin-dialog';
	import { goto } from '$app/navigation';
	import type { AppSnapshot, IndexState, ProviderStatus } from '$lib/types';
	import { theme, toggleTheme } from '$lib/theme';
	import { chatSidebarShortcut } from '$lib/stores';
	import { prettyShortcut, shortcutFromEvent, shortcutsCollide } from '$lib/keyboardShortcut';

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
	async function applyShortcut(combo: string) {
		if (shortcutsCollide(combo, $chatSidebarShortcut)) {
			quickShortcutError = 'This shortcut is already assigned to the chat sidebar.';
			return;
		}
		try {
			await invoke('set_quick_shortcut', { shortcut: combo });
			quickShortcut = combo;
			quickShortcutError = '';
		} catch (e) {
			quickShortcutError = String(e);
		}
	}
	function startRecording() {
		if (quickRecording) return;
		quickRecording = true;
		quickShortcutError = '';
		const MODS = [
			'ControlLeft',
			'ControlRight',
			'AltLeft',
			'AltRight',
			'ShiftLeft',
			'ShiftRight',
			'MetaLeft',
			'MetaRight',
			'OSLeft',
			'OSRight'
		];
		const cleanup = () => {
			quickRecording = false;
			window.removeEventListener('keydown', onKey, true);
		};
		const onKey = (e: KeyboardEvent) => {
			e.preventDefault();
			e.stopPropagation();
			if (MODS.includes(e.code)) return; // wait for a non-modifier key
			if (e.code === 'Escape') {
				cleanup();
				return;
			} // Esc cancels
			const parts: string[] = [];
			if (e.ctrlKey) parts.push('Ctrl');
			if (e.altKey) parts.push('Alt');
			if (e.shiftKey) parts.push('Shift');
			if (e.metaKey) parts.push('Super');
			parts.push(e.code);
			cleanup();
			void applyShortcut(parts.join('+'));
		};
		window.addEventListener('keydown', onKey, true);
	}

	function startChatShortcutRecording() {
		if (chatShortcutRecording) return;
		chatShortcutRecording = true;
		chatShortcutError = '';
		const cleanup = () => {
			chatShortcutRecording = false;
			window.removeEventListener('keydown', onKey, true);
		};
		const onKey = (e: KeyboardEvent) => {
			e.preventDefault();
			e.stopPropagation();
			if (e.code === 'Escape') {
				cleanup();
				return;
			}
			const combo = shortcutFromEvent(e);
			if (!combo) {
				if (
					![
						'ControlLeft',
						'ControlRight',
						'AltLeft',
						'AltRight',
						'ShiftLeft',
						'ShiftRight',
						'MetaLeft',
						'MetaRight',
						'OSLeft',
						'OSRight'
					].includes(e.code)
				) {
					chatShortcutError = 'Use Ctrl, Alt, or Super with another key.';
				}
				return;
			}
			if (shortcutsCollide(combo, quickShortcut)) {
				chatShortcutError = 'This shortcut is already assigned to Quick Capture.';
				cleanup();
				return;
			}
			$chatSidebarShortcut = combo;
			chatShortcutError = '';
			cleanup();
		};
		window.addEventListener('keydown', onKey, true);
	}

	const hasGpuBuild = () =>
		installedBackends.some((b) => b === 'cuda' || b === 'vulkan' || b === 'metal');
	const backendLabel = (b: string) =>
		b === 'cuda' ? 'CUDA' : b === 'vulkan' ? 'Vulkan' : b === 'metal' ? 'Metal' : 'CPU';
	const recommendedBeeBackend = $derived(
		backendPreference === 'cpu'
			? 'cpu'
			: downloadableBeeBackends.includes('metal')
				? 'metal'
				: nvidiaDetected && downloadableBeeBackends.includes('cuda')
					? 'cuda'
					: downloadableBeeBackends.includes('vulkan')
						? 'vulkan'
						: 'cpu'
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
		const query = aiConfigSearch.trim().toLowerCase();
		if (!query) return [] as number[];
		const source = aiConfigText.toLowerCase();
		const positions: number[] = [];
		let offset = 0;
		while (offset < source.length) {
			const found = source.indexOf(query, offset);
			if (found < 0) break;
			positions.push(found);
			offset = found + Math.max(query.length, 1);
		}
		return positions;
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

	onMount(async () => {
		try {
			await refreshAiConfig();
			await refreshSnapshot();
			const status = await loadProviderStatus();
			downloadableBackends = await invoke<string[]>('downloadable_backends');
			downloadableBeeBackends = await invoke<string[]>('downloadable_bee_backends');
			// Auto chooses based on hardware; the remaining values name the exact
			// backend. The retired generic GPU value is treated as Auto.
			const sp = status.config?.backendPreference;
			backendPreference =
				sp === 'cpu'
					? 'cpu'
					: sp === 'vulkan'
						? 'vulkan'
					: sp === 'metal'
						? 'metal'
						: sp === 'cuda'
							? 'cuda'
							: 'auto';
			thinking = status.config?.thinking ?? false;
			autoOffload = status.config?.autoOffload ?? true;
			deterministicTools = status.config?.deterministicTools ?? true;
			// Gating is opt-in and off by default (model-agnostic full toolset).
			toolGating = status.config?.toolGating ?? false;
			promptCache = status.config?.promptCache ?? true;
			recommendedThreads = status.recommendedThreads ?? null;
			try {
				llamaCache = await invoke('llama_cache_status');
			} catch (e) {
				console.error(e);
			}
			searxngUrl = (await invoke<string | null>('get_searxng_url')) ?? '';
			embedModelPath = (await invoke<string | null>('get_embed_model_path')) ?? '';
			quickShortcut = (await invoke<string>('get_quick_shortcut')) || 'Ctrl+Space';
			startWithSystem = (await invoke<{ startWithSystem: boolean }>('get_background_settings'))
				.startWithSystem;
			try {
				const oh = await invoke<OpenharnSettings>('get_openharn_settings');
				ohPort = oh.port ?? null;
				ohBinPath = oh.bin_path ?? '';
				ohToolMode = oh.tool_mode === 'native' || oh.tool_mode === 'prompt' ? oh.tool_mode : 'auto';
				// Manual grammar toggles only apply in explicit Prompt tools mode.
				// Do not display stale legacy values as active in Auto/Native.
				ohStrict = ohToolMode === 'prompt' ? (oh.strict ?? false) : false;
				ohPromptTools = false;
				ohCallOnly = ohToolMode === 'prompt' ? (oh.call_only ?? false) : false;
				ohNoThink = oh.no_think ?? false;
				ohToolChoice = oh.tool_choice ?? '';
				ohTemplateKwargs = oh.template_kwargs ?? '';
				ohMaxCalls = oh.max_calls ?? null;
				ohTotalMax = oh.total_max ?? null;
				ohToolTimeout = oh.tool_timeout_secs ?? null;
				ohBaseUrl = oh.base_url ?? '';
				externalEnabled = oh.external_enabled ?? false;
				externalBaseUrl = oh.external_base_url ?? '';
				externalModel = oh.external_model ?? '';
				externalApiKey = oh.external_api_key ?? '';
			} catch (e) {
				console.error('Failed to load openharn settings:', e);
			}
			try {
				modelProfiles = await invoke<ProfileInfo[]>('list_model_profiles');
			} catch (e) {
				modelProfiles = [];
			}
			if (status.resolved) {
				currentModelPath = status.config?.modelPath || status.resolved.modelPath || '';
				contextSize = status.config?.contextSize ?? status.resolved.contextSize ?? null;
				gpuLayers = status.config?.gpuLayers ?? status.resolved.gpuLayers ?? null;
				threads = status.config?.threads ?? status.resolved.threads ?? null;
				temperature = status.config?.temperature ?? status.resolved.temperature ?? null;
				topP = status.config?.topP ?? status.resolved.topP ?? null;
				maxTurns = status.config?.maxTurns ?? null;
				extraArgs = status.config?.extraArgs ?? status.resolved.extraArgs ?? [];
			} else if (status.config) {
				currentModelPath = status.config.modelPath || '';
				contextSize = status.config.contextSize ?? null;
				gpuLayers = status.config.gpuLayers ?? null;
				threads = status.config.threads ?? null;
				temperature = status.config.temperature ?? null;
				topP = status.config.topP ?? null;
				maxTurns = status.config.maxTurns ?? null;
				extraArgs = status.config.extraArgs ?? [];
			}

			enableJupyterExecution = localStorage.getItem('myelin_jupyter_exec') === 'true';

			// LaTeX support bundle: current cache state + live download progress.
			try {
				latexCache = await invoke('tectonic_cache_status');
			} catch (e) {
				console.error(e);
			}
			await listen<{ phase: string; bytes?: number; message?: string }>(
				'latex://download',
				async (event) => {
					const p = event.payload;
					if (p.phase === 'start') {
						latexDownloading = true;
						latexError = '';
						latexDownloadBytes = p.bytes ?? 0;
					} else if (p.phase === 'progress') {
						latexDownloading = true;
						latexDownloadBytes = p.bytes ?? latexDownloadBytes;
					} else if (p.phase === 'done') {
						latexDownloading = false;
						latexDownloadBytes = p.bytes ?? latexDownloadBytes;
						try {
							latexCache = await invoke('tectonic_cache_status');
						} catch (e) {
							console.error(e);
						}
					} else if (p.phase === 'error') {
						latexDownloading = false;
						latexError = p.message ?? 'Download failed';
					}
				}
			);

			// Live-update the backend badge when a server actually starts.
			await listen<{
				backend: string;
				engine: 'llama_cpp' | 'beellama';
				gpuOffloaded: boolean;
				fellBackToCpu: boolean;
			}>('ai://llama_backend', (event) => {
				activeBackend = event.payload.backend;
				activeEngine = event.payload.engine;
				backendFellBack = event.payload.fellBackToCpu;
			});

			// Backend download progress.
			await listen<{ backend: string; phase: string; percent: number; message: string }>(
				'backend://download',
				async (event) => {
					download = event.payload;
					if (event.payload.phase === 'done') {
						beeDownloadActive = false;
						await loadProviderStatus();
						setTimeout(() => {
							if (download?.phase === 'done') download = null;
						}, 4000);
					}
					if (event.payload.phase === 'error') beeDownloadActive = false;
				}
			);

			// Keep the "Running on" badge live (server may start/restart/crash
			// while this page is open).
			statusPoll = setInterval(() => {
				loadProviderStatus().catch(() => {});
			}, 2500);
		} catch (e) {
			console.error('Failed to load provider status:', e);
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
			const extraArgsArray = extraArgs.filter((arg) => arg.trim() !== '');
			await invoke('set_llama_advanced_config', {
				contextSize: contextSize,
				gpuLayers: gpuLayers,
				threads: threads,
				temperature: temperature,
				topP: topP,
				extraArgs: extraArgsArray.length > 0 ? extraArgsArray : null,
				backendPreference: backendPreference,
				gpuDevice: null,
				thinking: thinking,
				autoOffload: autoOffload,
				maxTurns: maxTurns
			});
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
			await invoke('set_openharn_settings', {
				settings: {
					port: ohPort || null,
					bin_path: ohBinPath.trim() || null,
					tool_mode: ohToolMode,
					strict: ohStrict,
					prompt_tools: ohPromptTools,
					call_only: ohCallOnly,
					no_think: ohNoThink,
					tool_choice: ohToolChoice.trim() || null,
					template_kwargs: ohTemplateKwargs.trim() || null,
					max_calls: ohMaxCalls || null,
					total_max: ohTotalMax || null,
					tool_timeout_secs: ohToolTimeout || null,
					base_url: ohBaseUrl.trim() || null,
					external_enabled: externalEnabled,
					external_base_url: externalBaseUrl.trim() || null,
					external_model: externalModel.trim() || null,
					external_api_key: externalApiKey.trim() || null
				}
			});
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
</script>

<div class="settings-container">
	<header class="settings-header">
		<button class="back-btn" onclick={goBack}>
			<svg
				viewBox="0 0 24 24"
				width="20"
				height="20"
				stroke="currentColor"
				stroke-width="2"
				fill="none"
				stroke-linecap="round"
				stroke-linejoin="round"
			>
				<line x1="19" y1="12" x2="5" y2="12"></line>
				<polyline points="12 19 5 12 12 5"></polyline>
			</svg>
			Back to Notes
		</button>
		<h1>Settings</h1>
	</header>

	<div class="settings-content">
		<section class="settings-section">
			<h2>Workspace</h2>
			<div class="info-grid">
				<div class="info-card">
					<span class="info-label">Path</span>
					<span class="info-value">{activeWorkspacePath || '—'}</span>
				</div>
				<div class="info-card">
					<span class="info-label">Index</span>
					<span class="info-value"
						>{indexState ? `${indexState.backend}:${indexState.noteCount} notes` : '—'}</span
					>
				</div>
				<div class="info-card">
					<span class="info-label">Provider</span>
					<span class="info-value">{activeProvider || '—'}</span>
				</div>
			</div>
			<div class="ws-actions">
				<button class="browse-btn" onclick={changeWorkspace}>Change workspace</button>
				<button class="browse-btn" onclick={rebuildIndex} disabled={isRebuilding}>
					{isRebuilding ? 'Rebuilding…' : 'Rebuild index'}
				</button>
			</div>
		</section>

		<section class="settings-section">
			<h2>AI Configuration File</h2>
			<p class="description">
				Technical AI settings are managed in a versioned JSON file so custom llama-server runtimes
				and model profiles can be changed without adding fragile UI controls.
			</p>
			<div class="info-grid">
				<div class="info-card"><span class="info-label">Status</span><span class="info-value">{aiConfig?.validationState ?? '—'}</span></div>
				<div class="info-card"><span class="info-label">Profile</span><span class="info-value">{aiConfig?.activeProfile ?? '—'}</span></div>
				<div class="info-card"><span class="info-label">Runtime</span><span class="info-value">{aiConfig?.runtimeId ?? '—'}</span></div>
			</div>
			<p class="compute-hint">{aiConfig?.configPath ?? 'AI config path unavailable'}{aiConfig?.hasUnappliedChanges ? ' · unapplied changes' : ''}</p>
			{#if aiConfig?.errors?.length}
				<div class="error-box">{#each aiConfig.errors as error}<div><code>{error.path}</code>: {error.message}</div>{/each}</div>
			{/if}
			{#if aiConfigMessage}<p class="compute-hint">{aiConfigMessage}</p>{/if}
			<div class="ws-actions">
				<button class="browse-btn" onclick={openAiConfig} disabled={!aiConfig}>Open config</button>
				<button class="browse-btn" onclick={copyAiConfigPath} disabled={!aiConfig}>Copy path</button>
				<button class="browse-btn" onclick={validateAiConfig} disabled={aiConfigBusy}>Validate</button>
				<button class="browse-btn" onclick={applyAiConfig} disabled={aiConfigBusy || !aiConfig?.candidateHash || aiConfig.validationState !== 'valid'}>Apply</button>
			</div>
		</section>

		<section class="settings-section">
			<h2>External AI Provider</h2>
			<p class="description">
				Use a hosted or separately running OpenAI-compatible model for simple Chat and Write requests.
				This is independent of the local runtime JSON configuration and does not use Myelin's llama.cpp
				slots or section KV caches.
			</p>
			<label class="toggle-row">
				<input type="checkbox" bind:checked={externalEnabled} onchange={saveOpenharn} />
				<span class="toggle-text">
					<strong>Use external model for Chat and Write</strong>
					<span class="toggle-hint">Turn off to return to the active local model profile.</span>
				</span>
			</label>
			<div class="advanced-grid">
				<div class="input-group">
					<label for="external_base_url">API base URL</label>
					<input
						type="url"
						id="external_base_url"
						bind:value={externalBaseUrl}
						onchange={saveOpenharn}
						placeholder="https://api.openai.com/v1"
					/>
				</div>
				<div class="input-group">
					<label for="external_model">Model name</label>
					<input
						type="text"
						id="external_model"
						bind:value={externalModel}
						onchange={saveOpenharn}
						placeholder="gpt-4o-mini or local-model-name"
					/>
				</div>
				<div class="input-group full-width">
					<label for="external_api_key">API key</label>
					<input
						type="password"
						id="external_api_key"
						bind:value={externalApiKey}
						onchange={saveOpenharn}
						placeholder="Optional for local compatible servers"
						autocomplete="off"
					/>
					<p class="compute-hint">Stored locally in Myelin's settings.json and omitted from debug prompts.</p>
				</div>
			</div>
			<p class="compute-hint">
				The base URL should normally end in <code>/v1</code>. Chat works with text-only providers; Write
				requires compatible tool/function calling.
			</p>
		</section>

		<div style="display: none">
		{#if true}
		<section class="settings-section">
			<h2>Local AI Model Configuration</h2>
			<p class="description">
				Select a model to use for local AI features. It runs completely offline on your device and
				is saved in app settings, not inside the notes workspace. <strong
					>Only <code>.gguf</code> models are supported</strong
				> (llama.cpp format).
			</p>

			<div class="model-picker">
				<div class="path-display" class:empty={!currentModelPath}>
					{currentModelPath || 'No model selected'}
				</div>
				<button class="browse-btn" onclick={selectModel} disabled={isSaving}> Browse... </button>
			</div>

			<div class="compute-device">
				<span class="compute-label">Inference engine</span>
				<div class="segmented" role="group" aria-label="Inference engine">
					<button
						type="button"
						class="segment"
						class:active={inferenceEngine === 'llama_cpp'}
						onclick={() => selectInferenceEngine('llama_cpp')}
					>
						llama.cpp · Stable
					</button>
					<button
						type="button"
						class="segment"
						class:active={inferenceEngine === 'beellama'}
						disabled={beeDownloadActive}
						onclick={() => selectInferenceEngine('beellama')}
					>
						{beeDownloadActive
							? 'Installing BeeLlama…'
							: installedBeeBackends.includes(recommendedBeeBackend)
								? 'BeeLlama · Experimental'
								: `Download BeeLlama ${backendLabel(recommendedBeeBackend)}`}
					</button>
				</div>
				<p class="compute-hint">
					BeeLlama v0.4.1 is an experimental, drop-in llama-server fork. Prompt and persistent slot
					caching remain enabled. Stock llama.cpp is used automatically if Bee fails to start.
				</p>
				{#if inferenceEngine === 'beellama'}
					<div class="device-issue warn">
						<span class="issue-icon">⚠️</span>
						<span>
							Experimental engine selected.
							{activeEngine === 'llama_cpp'
								? ' BeeLlama failed or is unavailable, so the stable engine is active.'
								: ` Active backend: ${activeBackend?.toUpperCase() ?? 'not started yet'}.`}
						</span>
					</div>
				{/if}
			</div>

			<div class="compute-device">
				<span class="compute-label">Compute device</span>
				<div class="segmented" role="group" aria-label="Compute device">
					{#each [{ value: 'auto', label: 'Auto' }, { value: 'cuda', label: 'CUDA' }, { value: 'vulkan', label: 'Vulkan' }, ...(downloadableBackends.includes('metal') ? [{ value: 'metal', label: 'Metal' }] : []), { value: 'cpu', label: 'CPU' }] as opt}
						{@const disabled = opt.value !== 'auto' && opt.value !== 'cpu' && !installedBackends.includes(opt.value)}
						<button
							type="button"
							class="segment"
							class:active={backendPreference === opt.value}
							{disabled}
							title={disabled ? `No ${opt.label} build installed` : ''}
							onclick={() => selectBackend(opt.value as BackendPref)}
						>
							{opt.label}
						</button>
					{/each}
				</div>
				<p class="compute-hint">
					{#if backendPreference === 'cpu'}
						Most reliable: runs entirely on the CPU. Works with every model (some models give wrong
						output on the GPU), at the cost of speed.
					{:else if backendPreference === 'vulkan'}
						Power-saving: runs on the integrated GPU via Vulkan. The app still manages offload and
						falls back to CPU if needed.
					{:else if backendPreference === 'cuda'}
						NVIDIA GPU acceleration through CUDA. Requires the installed CUDA backend.
					{:else if backendPreference === 'metal'}
						macOS GPU acceleration: runs through Apple Metal. Requires the downloaded Metal backend
						and falls back to CPU if needed.
					{:else}
						Recommended: detects your hardware and picks the fastest backend — dedicated GPU (CUDA),
						integrated GPU (Vulkan), or CPU — and falls back on its own.
					{/if}
				</p>

				{#if gpuIssue}
					<div class="device-issue warn">
						<span class="issue-icon">⚠️</span>
						<span>{gpuIssue.message}</span>
					</div>
				{/if}

				{#if !providerHealthy && providerDetail}
					<div class="device-issue error">
						<span class="issue-icon">⛔</span>
						<span>{providerDetail}</span>
					</div>
				{/if}
			</div>

			<div
				class="backend-status"
				class:gpu={computeStatus.level === 'gpu'}
				class:cpu={computeStatus.level === 'cpu'}
			>
				<span class="backend-dot"></span>
				<div class="backend-text">
					<strong>{computeStatus.title}</strong>
					<span>{computeStatus.detail}</span>
				</div>
			</div>

			{#if downloadableBackends.length > 0}
				<div class="backends-list">
					<span class="compute-label">Installed backends</span>
					{#each downloadableBackends as b}
						{@const installed = installedBackends.includes(b)}
						{@const busy =
							download?.backend === b && download?.phase !== 'done' && download?.phase !== 'error'}
						<div class="backend-item">
							<span class="backend-name">{backendLabel(b)}</span>
							{#if busy}
								<div class="backend-progress">
									<div class="backend-bar">
										<div class="backend-bar-fill" style="width:{download?.percent ?? 0}%"></div>
									</div>
									<span class="backend-progress-text"
										>{download?.message ?? ''} ({Math.round(download?.percent ?? 0)}%)</span
									>
								</div>
							{:else if installed}
								<span class="backend-installed">✓ Installed</span>
							{:else}
								<button
									class="browse-btn"
									onclick={() => downloadBackend(b)}
									disabled={!!download && download.phase !== 'done' && download.phase !== 'error'}
								>
									Download
								</button>
							{/if}
						</div>
					{/each}
					{#if download?.phase === 'error'}
						<div class="device-issue error">
							<span class="issue-icon">⛔</span>
							<span>Download failed: {download.message}</span>
						</div>
					{/if}
					<p class="compute-hint">
						CPU and Vulkan ship with the app where supported. On macOS, download Metal for Apple GPU
						acceleration; on Windows, download CUDA for NVIDIA GPUs.
					</p>
				</div>
			{/if}

			<br />
			<h2>Web Search</h2>
			<p class="description">
				Privacy-first web search for the assistant. Set a SearXNG instance URL, or leave blank to
				use the no-key DuckDuckGo fallback.
			</p>
			<div class="model-picker">
				<input
					type="text"
					class="path-display"
					bind:value={searxngUrl}
					placeholder="https://searx.example.org (optional)"
					onchange={saveSearxng}
				/>
			</div>

			<br />
			<h2>Embeddings — Document Search & RAG</h2>
			<p class="description">
				A small embedding model (e.g. nomic-embed-text v1.5, Q8_0) powers semantic search over your
				notes and lets the assistant search ingested documents (PDFs, books). It runs as a second
				local server.
			</p>
			<div class="model-picker">
				<div class="path-display" class:empty={!embedModelPath}>
					{embedModelPath ||
						'No embedding model selected — semantic search uses a lexical fallback'}
				</div>
				<button class="browse-btn" onclick={pickEmbedModel}>Browse...</button>
				{#if embedModelPath}
					<button class="browse-btn" onclick={clearEmbedModel}>Clear</button>
				{/if}
			</div>

			{#if modelProfiles.length > 0}
				<br />
				<h2>Compatible Models</h2>
				<p class="description">
					Models with a verified profile work out of the box (tool-calling and chat template tuned).
					Other models run on auto-detected defaults.
				</p>
				<div class="backends-list">
					{#each modelProfiles as p}
						<div class="backend-item">
							<span class="backend-name">
								{p.name}{#if p.role === 'embed'}
									<small>(embedding)</small>{/if}
							</span>
							{#if p.verified}
								<span class="backend-installed">✓ Verified</span>
							{:else}
								<span class="backend-progress-text">Experimental</span>
							{/if}
						</div>
						{#if p.notes}
							<p class="compute-hint">{p.notes}</p>
						{/if}
					{/each}
				</div>
			{/if}

			<br />
			<h2>Advanced AI Configuration</h2>
			<p class="description">
				Fine-tune llama-server memory usage and CLI flags. Leave blank to use system defaults.
			</p>
			<label class="toggle-row">
				<input
					type="checkbox"
					bind:checked={promptCache}
					onchange={async () => {
						await invoke('set_prompt_cache', { enabled: promptCache });
						llamaCache = await invoke('llama_cache_status');
					}}
				/>
				<span class="toggle-text">
					<strong>Persistent per-note prompt cache</strong>
					<span class="toggle-hint">
						Reuses llama.cpp slot snapshots across restarts. No automatic retention limit.
						{#if llamaCache}
							({formatMB(llamaCache.sizeBytes)} cached){/if}
					</span>
				</span>
			</label>
			<button
				class="secondary"
				onclick={async () => {
					await invoke('clear_llama_cache');
					llamaCache = await invoke('llama_cache_status');
				}}>Clear prompt cache</button
			>
			<label class="toggle-row">
				<input type="checkbox" bind:checked={autoOffload} onchange={debounceSave} />
				<span class="toggle-text">
					<strong>Adaptive GPU offload (recommended)</strong>
					<span class="toggle-hint">
						{autoOffload
							? 'On — automatically uses available VRAM, keeps the KV cache in RAM for a large (32k) context, and retries with less if the GPU runs out. Manages Context Size & GPU Layers for you.'
							: 'Off — use the manual Context Size & GPU Layers below exactly as set.'}
					</span>
				</span>
			</label>

			<div class="advanced-grid">
				<div class="input-group">
					<label for="ctx">Context Size {autoOffload ? '(auto)' : ''}</label>
					<input
						type="number"
						id="ctx"
						bind:value={contextSize}
						oninput={debounceSave}
						placeholder="auto"
						disabled={autoOffload}
					/>
				</div>
				<div class="input-group">
					<label for="ngl">GPU Layers {autoOffload ? '(auto)' : ''}</label>
					<input
						type="number"
						id="ngl"
						bind:value={gpuLayers}
						oninput={debounceSave}
						placeholder="auto"
						disabled={autoOffload}
					/>
				</div>
				<div class="input-group">
					<label for="threads">CPU Threads</label>
					<input
						type="number"
						id="threads"
						bind:value={threads}
						oninput={debounceSave}
						placeholder={recommendedThreads
							? `Auto — ${recommendedThreads} physical cores`
							: 'Auto'}
					/>
					<span class="toggle-hint">
						{threads
							? `Explicit override: ${threads} threads`
							: recommendedThreads
								? `Auto uses ${recommendedThreads} physical cores`
								: 'Auto uses the detected physical cores'}
					</span>
				</div>
				<div class="input-group">
					<label for="temp">Temperature</label>
					<input
						type="number"
						step="0.1"
						id="temp"
						bind:value={temperature}
						oninput={debounceSave}
						placeholder="0.2"
					/>
				</div>
				<div class="input-group">
					<label for="top_p">Top P</label>
					<input
						type="number"
						step="0.05"
						id="top_p"
						bind:value={topP}
						oninput={debounceSave}
						placeholder="0.95"
					/>
				</div>
				<div class="input-group">
					<label for="max_turns">Max Tool Turns</label>
					<input
						type="number"
						min="1"
						max="12"
						step="1"
						id="max_turns"
						bind:value={maxTurns}
						oninput={debounceSave}
						placeholder="4"
					/>
				</div>
			</div>

			<label class="toggle-row">
				<input type="checkbox" bind:checked={thinking} onchange={debounceSave} />
				<span class="toggle-text">
					<strong>Model thinking / reasoning</strong>
					<span class="toggle-hint">
						{thinking
							? 'On — the model reasons before answering (slower, may be more accurate).'
							: 'Off — faster, no hidden reasoning tokens. Works across models.'}
					</span>
				</span>
			</label>

			<div class="input-group full-width" style="margin-top: 1rem;">
				<label>
					Extra Arguments
					<div style="font-size: 0.8em; color: var(--text-error); margin-top: 4px;">
						<strong>CRITICAL NOTE:</strong> Because of how system processes work, you cannot put
						spaces in a single box! If you wanted to add <code>--threads 8</code>, you must put
						<code>--threads</code>
						in one box, click add again, and put <code>8</code> in the next box!
					</div>
				</label>
				{#each extraArgs as arg, i}
					<div style="display: flex; gap: var(--space-2); margin-bottom: var(--space-2);">
						<input
							type="text"
							bind:value={extraArgs[i]}
							oninput={debounceSave}
							placeholder="--flash-attn"
							style="flex: 1;"
						/>
						<button
							class="browse-btn"
							onclick={() => removeExtraArg(i)}
							title="Remove argument"
							style="padding: 0 1rem; color: #f87171; border-color: rgba(248, 113, 113, 0.3);"
						>
							Remove
						</button>
					</div>
				{/each}
				<button
					class="browse-btn"
					onclick={addExtraArg}
					style="align-self: flex-start; margin-top: 4px;"
				>
					+ Add Argument
				</button>
			</div>
		</section>

		<section class="settings-section">
			<h2>Assistant Tooling</h2>
			<p class="description">
				Two independent assists for how the model uses tools. Hover each <span class="info-dot"
					>i</span
				> for details and caveats.
			</p>

			<label class="toggle-row">
				<input
					type="checkbox"
					bind:checked={toolGating}
					onchange={() => invoke('set_tool_gating', { enabled: toolGating })}
				/>
				<span class="toggle-text">
					<span class="toggle-label">
						<strong>Per-message tool gating</strong>
						<span class="info" tabindex="0" role="note" aria-label="About per-message tool gating">
							<span class="info-dot">i</span>
							<span class="info-pop"
								>Offers the model only the tools its message seems to need, chosen by keyword
								heuristics — brittle and not model-agnostic. It can <strong
									>withhold a tool the model would have used</strong
								>: e.g. “search for the latest news” isn’t recognised as a web search, so the model
								can’t search at all. <strong>Off by default</strong> — only useful for sub-2B models that
								misfire on tools they shouldn’t touch.</span
							>
						</span>
					</span>
					<span class="toggle-hint">
						{toolGating
							? 'On — only the tools your message seems to need are offered each turn.'
							: 'Off — full toolset every turn, the model decides (recommended).'}
					</span>
				</span>
			</label>

			<label class="toggle-row">
				<input
					type="checkbox"
					bind:checked={deterministicTools}
					onchange={() => invoke('set_deterministic_tools', { enabled: deterministicTools })}
				/>
				<span class="toggle-text">
					<span class="toggle-label">
						<strong>Deterministic format &amp; find</strong>
						<span
							class="info"
							tabindex="0"
							role="note"
							aria-label="About deterministic format and find"
						>
							<span class="info-dot">i</span>
							<span class="info-pop"
								>In-code correctness assists that don’t withhold tools — they make the result
								reliable: formatting (strip headings/bold/bullets, change case, convert lists) is
								applied by exact rules instead of the model rewriting the whole note; exact-word
								lookups use a reliable search; and a guard prevents accidentally wiping a note
								during an edit. <strong>On by default.</strong> (Surgical deletes — remove a paragraph/heading/section
								— always apply, regardless of this toggle.)</span
							>
						</span>
					</span>
					<span class="toggle-hint">
						{deterministicTools
							? 'On — reliable in-code formatting, search & a wipe guard.'
							: 'Off — the model handles formatting & edits on its own.'}
					</span>
				</span>
			</label>
		</section>

		<section class="settings-section">
			<h2>Agent (openharn)</h2>
			<p class="description">
				Myelin runs the openharn agent harness as a sidecar process that drives the local model and
				calls back into Myelin for the real note / search / web tools. These settings tune that
				sidecar. The sidecar binary is required for AI agent and tool-calling features. Run <code
					>npm run build:sidecar</code
				> before development or packaging, or choose an existing binary below. Leave the path blank to
				use the bundled/resource-dir lookup.
			</p>

			<div class="input-group full-width">
				<label for="oh_tool_mode">Tool-calling strategy</label>
				<select id="oh_tool_mode" bind:value={ohToolMode} onchange={changeToolMode}>
					<option value="auto">Auto — choose per request</option>
					<option value="native">Native — use the model's function calls</option>
					<option value="prompt">Prompt tools — text-form calls with grammar options</option>
				</select>
				<p class="compute-hint">
					Auto uses native calls for simple requests and prompt tools only when Openharn's
					per-request policy needs them. Native is usually best for larger models. Prompt tools can
					help smaller or unreliable models, but may reduce quality on larger models.
				</p>
			</div>

			<label class="toggle-row">
				<input
					type="checkbox"
					bind:checked={ohStrict}
					onchange={saveOpenharn}
					disabled={ohToolMode !== 'prompt'}
				/>
				<span class="toggle-text">
					<strong>Strict tool grammar</strong>
					<span class="toggle-hint"
						>Restricts text-form tool calls to valid structured syntax. More reliable, but less
						flexible and slower.</span
					>
				</span>
			</label>

			<label class="toggle-row">
				<input
					type="checkbox"
					bind:checked={ohCallOnly}
					onchange={saveOpenharn}
					disabled={ohToolMode !== 'prompt'}
				/>
				<span class="toggle-text">
					<strong>Call-only tool requests</strong>
					<span class="toggle-hint"
						>Prevents prose when a request is classified as an operation. Useful for weak models;
						can be too restrictive for larger models.</span
					>
				</span>
			</label>

			<label class="toggle-row">
				<input type="checkbox" bind:checked={ohNoThink} onchange={saveOpenharn} />
				<span class="toggle-text">
					<strong>Disable model reasoning</strong>
					<span class="toggle-hint"
						>Adds a request hint to skip hidden thinking tokens. This can make responses faster, but
						some models may become less accurate.</span
					>
				</span>
			</label>

			<div class="advanced-grid">
				<div class="input-group">
					<label for="oh_tool_choice">Native tool choice</label>
					<select id="oh_tool_choice" bind:value={ohToolChoice} onchange={saveOpenharn}>
						<option value="">Auto</option>
						<option value="required">Required — force a native tool call</option>
						<option value="none">None — disable native tool calls</option>
					</select>
				</div>
				<div class="input-group">
					<label for="oh_template_kwargs">Chat-template options (JSON)</label>
					<input
						type="text"
						id="oh_template_kwargs"
						bind:value={ohTemplateKwargs}
						onchange={saveOpenharn}
						placeholder="JSON, e.g. enable_thinking false"
					/>
				</div>
			</div>

			<div class="advanced-grid">
				<div class="input-group">
					<label for="oh_port">Sidecar port</label>
					<input
						type="number"
						id="oh_port"
						bind:value={ohPort}
						onchange={saveOpenharn}
						placeholder="8091"
					/>
				</div>
				<div class="input-group">
					<label for="oh_maxcalls">Max tool calls / turn</label>
					<input
						type="number"
						min="1"
						id="oh_maxcalls"
						bind:value={ohMaxCalls}
						onchange={saveOpenharn}
						placeholder="auto"
					/>
				</div>
				<div class="input-group">
					<label for="oh_totalmax">Max tool calls / chat</label>
					<input
						type="number"
						min="1"
						id="oh_totalmax"
						bind:value={ohTotalMax}
						onchange={saveOpenharn}
						placeholder="auto"
					/>
				</div>
				<div class="input-group">
					<label for="oh_timeout">Tool timeout (s)</label>
					<input
						type="number"
						min="1"
						id="oh_timeout"
						bind:value={ohToolTimeout}
						onchange={saveOpenharn}
						placeholder="300"
					/>
				</div>
			</div>

			<div class="model-picker">
				<input
					type="text"
					class="path-display"
					bind:value={ohBaseUrl}
					placeholder="llama-server base URL override, e.g. http://127.0.0.1:39281/v1"
					onchange={saveOpenharn}
				/>
			</div>
			<p class="compute-hint">
				Override the llama-server URL the sidecar calls. Blank = the model server Myelin is already
				configured to use.
			</p>

			<div class="model-picker">
				<div class="path-display" class:empty={!ohBinPath}>
					{ohBinPath || 'Bundled openharn-myelin (auto-detected)'}
				</div>
				<button class="browse-btn" onclick={pickOpenharnBin} disabled={ohSaving}> Browse… </button>
			</div>
			<p class="compute-hint">
				Explicit path to the sidecar binary. Blank = bundled/resource-dir lookup. If it is missing,
				run <code>npm run build:sidecar</code> or set <code>OPENHARN_MYELIN_BIN</code>.
			</p>

			<p class="compute-hint">
				Tool-calling format, intent detection, and reasoning behavior are selected automatically per
				request from the active model profile and interaction mode.
			</p>
		</section>

		{/if}
		</div>
		<section class="settings-section">
			<h2>Appearance</h2>
			<div
				class="feature-toggle"
				style="display: flex; justify-content: space-between; align-items: center; margin-top: 1rem;"
			>
				<div>
					<h3 style="margin: 0; font-size: 1rem;">Theme</h3>
					<p class="description" style="margin-top: 4px;">
						Switch between the dark and light interface. Your choice is remembered across sessions.
					</p>
				</div>
				<button class="browse-btn" onclick={toggleTheme}>
					{$theme === 'light' ? 'Light' : 'Dark'}
				</button>
			</div>
		</section>

		<section class="settings-section">
			<h2>Features</h2>
			<div
				class="feature-toggle"
				style="display: flex; justify-content: space-between; align-items: center; margin-top: 1rem;"
			>
				<div>
					<h3 style="margin: 0; font-size: 1rem;">Jupyter Code Execution</h3>
					<p class="description" style="margin-top: 4px;">
						Allow execution of Python code cells within `.ipynb` notebooks using your local Python
						installation.
					</p>
				</div>
				<button class="browse-btn" onclick={toggleJupyterExecution}>
					{enableJupyterExecution ? 'Enabled' : 'Disabled'}
				</button>
			</div>
		</section>

		<section class="settings-section">
			<h2>Keyboard Shortcuts</h2>
			<div
				class="feature-toggle"
				style="display: flex; justify-content: space-between; align-items: center; margin-top: 1rem; gap: 1rem;"
			>
				<div>
					<h3 style="margin: 0; font-size: 1rem;">Chat sidebar</h3>
					<p class="description" style="margin-top: 4px;">
						{#if chatShortcutError}
							<span style="color: var(--danger, #e5534b);">{chatShortcutError}</span>
						{:else if chatShortcutRecording}
							Press your shortcut… (Esc to cancel)
						{:else}
							Current: <strong>{prettyShortcut($chatSidebarShortcut)}</strong>
						{/if}
					</p>
				</div>
				<button
					class="browse-btn"
					onclick={startChatShortcutRecording}
					disabled={chatShortcutRecording}
				>
					{chatShortcutRecording ? 'Recording…' : 'Change'}
				</button>
			</div>
		</section>

		<section class="settings-section">
			<h2>Quick Capture</h2>
			<p class="description">
				A global shortcut opens a small window to jot down a task from anywhere — even when Myelin
				isn't focused.
			</p>
			<div
				class="feature-toggle"
				style="display: flex; justify-content: space-between; align-items: center; margin-top: 1rem; gap: 1rem;"
			>
				<div>
					<h3 style="margin: 0; font-size: 1rem;">Global shortcut</h3>
					<p class="description" style="margin-top: 4px;">
						{#if quickShortcutError}
							<span style="color: var(--danger, #e5534b);">{quickShortcutError}</span>
						{:else if quickRecording}
							Press your shortcut… (Esc to cancel)
						{:else}
							Current: <strong>{prettyShortcut(quickShortcut)}</strong>
						{/if}
					</p>
				</div>
				<button class="browse-btn" onclick={startRecording} disabled={quickRecording}>
					{quickRecording ? 'Recording…' : 'Change'}
				</button>
			</div>
		</section>

		<section class="settings-section">
			<h2>Background</h2>
			<div
				class="feature-toggle"
				style="display: flex; justify-content: space-between; align-items: center; margin-top: 1rem; gap: 1rem;"
			>
				<div>
					<h3 style="margin: 0; font-size: 1rem;">Start Myelin with the system</h3>
					<p class="description" style="margin-top: 4px;">
						Starts hidden in the tray with the shortcut and model ready.
					</p>
					{#if backgroundError}<p class="description" style="color: var(--danger, #e5534b);">
							{backgroundError}
						</p>{/if}
				</div>
				<button
					class="browse-btn"
					onclick={async () => {
						const next = !startWithSystem;
						try {
							await invoke('set_start_with_system', { enabled: next });
							startWithSystem = next;
							backgroundError = '';
						} catch (e) {
							backgroundError = String(e);
						}
					}}
				>
					{startWithSystem ? 'Enabled' : 'Disabled'}
				</button>
			</div>
		</section>

		<section class="settings-section">
			<h2>LaTeX → PDF</h2>
			<p class="description">
				Compiling <code>.tex</code> notes to PDF uses Tectonic, which downloads a LaTeX support bundle
				(~50&nbsp;MB) on first use. Download it now to make the first compile instant and to work fully
				offline afterwards.
			</p>
			<div
				class="feature-toggle"
				style="display: flex; justify-content: space-between; align-items: center; margin-top: 1rem; gap: 1rem;"
			>
				<div>
					<h3 style="margin: 0; font-size: 1rem;">LaTeX support files</h3>
					<p class="description" style="margin-top: 4px;">
						{#if latexDownloading}
							Downloading… {formatMB(latexDownloadBytes)}
						{:else if latexError}
							<span style="color: var(--danger, #e5534b);">Error: {latexError}</span>
						{:else if latexCache?.warmed}
							Ready — {formatMB(latexCache.sizeBytes)} cached.
						{:else}
							Not downloaded yet.
						{/if}
					</p>
				</div>
				<button
					class="browse-btn"
					onclick={downloadLatexSupport}
					disabled={latexDownloading || latexCache?.warmed}
				>
					{#if latexDownloading}
						Downloading…
					{:else if latexCache?.warmed}
						Downloaded
					{:else}
						Download now
					{/if}
				</button>
			</div>
		</section>
	</div>
</div>

	<svelte:window onkeydown={handleAiConfigKeydown} />

{#if showAiConfig}
	<div class="config-modal-backdrop" role="presentation" onclick={(event) => { if (event.target === event.currentTarget) showAiConfig = false; }}>
		<section class="config-modal" role="dialog" aria-modal="true" aria-labelledby="ai-config-title">
			<header class="config-modal-header">
				<div><h2 id="ai-config-title">AI Configuration</h2><p>{aiConfig?.configPath}</p></div>
				<button class="icon-btn" aria-label="Close configuration" onclick={() => (showAiConfig = false)}>×</button>
			</header>
			<div class="config-search-row">
				<input class="config-search" bind:this={aiConfigSearchInput} bind:value={aiConfigSearch} oninput={() => (aiConfigSearchIndex = -1)} onkeydown={handleConfigSearchKeydown} placeholder="Search configuration (Ctrl+F)" aria-label="Search configuration" />
				{#if aiConfigSearch}<span>{configMatchCount() ? `${aiConfigSearchIndex + 1} / ${configMatchCount()}` : '0 matches'}</span>{/if}
				<button class="config-nav-btn" onclick={() => gotoConfigMatch(-1)} disabled={!configMatchCount()} aria-label="Previous match">↑</button>
				<button class="config-nav-btn" onclick={() => gotoConfigMatch(1)} disabled={!configMatchCount()} aria-label="Next match">↓</button>
			</div>
			<textarea class="config-editor" bind:this={aiConfigEditor} bind:value={aiConfigText} spellcheck="false" aria-label="AI configuration JSON"></textarea>
			<div class="config-modal-actions">
				<button class="browse-btn" onclick={() => (showAiConfig = false)}>Cancel</button>
				<button class="browse-btn" onclick={saveAiConfigText} disabled={aiConfigBusy}>Save</button>
				<button class="browse-btn primary" onclick={async () => { await saveAiConfigText(); await validateAiConfig(); }}>Save &amp; Validate</button>
			</div>
		</section>
	</div>
{/if}

{#if saved}
	<div class="success-message">
		<svg
			xmlns="http://www.w3.org/2000/svg"
			width="16"
			height="16"
			viewBox="0 0 24 24"
			fill="none"
			stroke="currentColor"
			stroke-width="2"
			stroke-linecap="round"
			stroke-linejoin="round"
		>
			<polyline points="20 6 9 17 4 12"></polyline>
		</svg>
		Settings saved successfully!
	</div>
{/if}

<style>
	.settings-container {
		display: flex;
		flex-direction: column;
		height: 100%;
		background: var(--bg-page);
		color: var(--text-primary);
		font-family: var(--font-sans);
		overflow-y: auto;
	}

	.settings-header {
		padding: var(--space-6) var(--space-8);
		border-bottom: 1px solid var(--border-default);
		display: flex;
		flex-direction: column;
		gap: var(--space-4);
		background: var(--bg-panel);
	}

	.back-btn {
		display: inline-flex;
		align-items: center;
		gap: var(--space-2);
		background: transparent;
		border: none;
		color: var(--text-secondary);
		font-family: var(--font-sans);
		font-size: 0.875rem;
		cursor: pointer;
		padding: 0;
		transition: color var(--duration-fast);
		align-self: flex-start;
	}

	.back-btn:hover {
		color: var(--text-primary);
	}

	.settings-header h1 {
		margin: 0;
		font-size: 2rem;
		font-weight: 600;
		color: var(--text-hero);
	}

	.settings-content {
		padding: var(--space-8);
		max-width: 800px;
		width: 100%;
		margin: 0 auto;
		display: flex;
		flex-direction: column;
		gap: var(--space-6);
	}

	.info-grid {
		display: grid;
		grid-template-columns: repeat(3, 1fr);
		gap: var(--space-3);
	}
	.info-card {
		padding: var(--space-4);
		background: var(--bg-page);
		border: 1px solid var(--border-default);
		border-radius: var(--radius-sm);
		display: flex;
		flex-direction: column;
		gap: var(--space-1);
	}
	.info-label {
		font-size: 0.6rem;
		font-weight: 700;
		text-transform: uppercase;
		letter-spacing: 0.09em;
		color: var(--neutral-600);
		font-family: var(--font-mono);
	}
	.info-value {
		font-size: 0.75rem;
		color: var(--neutral-300);
		word-break: break-all;
		font-family: var(--font-mono);
	}
	.ws-actions {
		display: flex;
		gap: var(--space-3);
	}

	.settings-section {
		background: var(--bg-panel);
		border: 1px solid var(--border-default);
		border-radius: var(--radius-md);
		padding: var(--space-6);
		display: flex;
		flex-direction: column;
		gap: var(--space-4);
	}
	.config-modal-backdrop {
		position: fixed; inset: 0; z-index: 100; display: grid; place-items: center;
		background: rgba(0, 0, 0, 0.68); padding: 2rem;
	}
	.config-modal {
		width: min(900px, 95vw); height: min(760px, 90vh); display: flex; flex-direction: column;
		background: var(--bg-modal); color: var(--text-primary); border: 1px solid var(--border-default);
		border-radius: 8px; box-shadow: 0 24px 80px rgba(0,0,0,.5); padding: 1rem;
	}
	.config-modal-header { display: flex; justify-content: space-between; align-items: start; gap: 1rem; }
	.config-modal-header h2 { margin: 0; }
	.config-modal-header p { margin: .35rem 0 1rem; color: var(--text-secondary); font-size: .8rem; word-break: break-all; }
	.config-editor { flex: 1; width: 100%; resize: none; box-sizing: border-box; padding: 1rem; color: var(--text-primary); background: var(--bg-input); border: 1px solid var(--border-default); border-radius: 5px; caret-color: var(--accent-100); font: 13px/1.5 ui-monospace, SFMono-Regular, Menlo, monospace; }
	.config-search-row { display: flex; align-items: center; gap: .6rem; margin-bottom: .6rem; color: var(--text-secondary); font-size: .8rem; }
	.config-search { flex: 1; padding: .5rem .65rem; color: var(--text-primary); background: var(--bg-input); border: 1px solid var(--border-default); border-radius: 4px; }
	.config-nav-btn { min-width: 2rem; padding: .35rem .55rem; color: var(--text-primary); background: var(--bg-elevated); border: 1px solid var(--border-default); border-radius: 4px; cursor: pointer; }
	.config-nav-btn:disabled { opacity: .4; cursor: default; }
	.config-modal-actions { display: flex; justify-content: flex-end; gap: .6rem; padding-top: 1rem; }
	.config-modal-actions .primary { background: var(--accent-100); color: #fff; }

	.settings-section h2 {
		margin: 0;
		font-size: 1.25rem;
		font-weight: 500;
		color: var(--text-hero);
		font-family: var(--font-sans);
	}

	.description {
		margin: 0;
		font-size: 0.875rem;
		color: var(--text-secondary);
		line-height: 1.5;
	}

	code {
		font-family: var(--font-mono);
		background: rgba(255, 255, 255, 0.1);
		padding: 0.1em 0.3em;
		border-radius: var(--radius-xs);
	}

	.model-picker {
		display: flex;
		gap: var(--space-3);
		margin-top: var(--space-2);
	}

	.path-display {
		flex: 1;
		background: var(--bg-page);
		border: 1px solid var(--border-default);
		border-radius: var(--radius-sm);
		padding: 0.75rem 1rem;
		font-family: var(--font-mono);
		font-size: 0.875rem;
		color: var(--text-primary);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
		display: flex;
		align-items: center;
	}

	.path-display.empty {
		color: var(--text-secondary);
		font-style: italic;
	}

	.compute-device {
		margin-top: var(--space-4);
	}

	.compute-label {
		display: block;
		font-size: 0.8rem;
		font-weight: 600;
		color: var(--text-secondary);
		margin-bottom: var(--space-2);
	}

	.segmented {
		display: inline-flex;
		border: 1px solid var(--border-default);
		border-radius: var(--radius-sm);
		overflow: hidden;
	}

	.segment {
		padding: 0.45rem 1.1rem;
		background: var(--bg-page);
		color: var(--text-secondary);
		border: none;
		border-right: 1px solid var(--border-default);
		font-size: 0.85rem;
		cursor: pointer;
		transition:
			background 0.12s,
			color 0.12s;
	}

	.segment:last-child {
		border-right: none;
	}

	.segment:hover {
		color: var(--text-primary);
	}

	.segment.active {
		background: var(--neutral-800);
		color: var(--text-primary);
		font-weight: 600;
	}

	.segment:disabled {
		opacity: 0.4;
		cursor: not-allowed;
	}

	.device-issue {
		display: flex;
		align-items: flex-start;
		gap: var(--space-2);
		margin-top: var(--space-3);
		padding: 0.6rem 0.9rem;
		border-radius: var(--radius-sm);
		border: 1px solid var(--border-default);
		font-size: 0.82rem;
		line-height: 1.45;
	}

	.device-issue .issue-icon {
		flex: 0 0 auto;
	}

	.device-issue.error {
		border-color: #b3402f;
		background: rgba(179, 64, 47, 0.1);
		color: var(--text-primary);
	}

	.device-issue.warn {
		border-color: #9a6b1f;
		background: rgba(154, 107, 31, 0.1);
		color: var(--text-primary);
	}

	.compute-hint {
		margin: var(--space-2) 0 0;
		font-size: 0.8rem;
		color: var(--text-secondary);
		line-height: 1.4;
	}

	.backend-status {
		display: flex;
		align-items: flex-start;
		gap: var(--space-2);
		margin-top: var(--space-3);
		padding: 0.6rem 0.9rem;
		border-radius: var(--radius-sm);
		border: 1px solid var(--border-default);
		font-size: 0.85rem;
		line-height: 1.4;
		color: var(--text-secondary);
	}

	.backend-text {
		display: flex;
		flex-direction: column;
		gap: 0.15rem;
	}

	.backend-text strong {
		color: var(--text-primary);
		font-weight: 600;
	}

	.backend-dot {
		flex: 0 0 auto;
		width: 8px;
		height: 8px;
		border-radius: 50%;
		margin-top: 0.35rem;
		background: var(--text-secondary);
	}

	.backends-list {
		margin-top: var(--space-4);
	}

	.backend-item {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--space-3);
		padding: 0.5rem 0;
		border-bottom: 1px solid var(--border-default);
	}

	.backend-name {
		font-size: 0.9rem;
		color: var(--text-primary);
		font-weight: 600;
	}

	.backend-installed {
		font-size: 0.82rem;
		color: #36c46f;
	}

	.backend-progress {
		display: flex;
		align-items: center;
		gap: var(--space-2);
		flex: 1;
		max-width: 70%;
	}

	.backend-bar {
		flex: 1;
		height: 6px;
		border-radius: 3px;
		background: var(--bg-page);
		overflow: hidden;
	}

	.backend-bar-fill {
		height: 100%;
		background: var(--accent, #e8500f);
		transition: width 0.2s;
	}

	.backend-progress-text {
		font-size: 0.75rem;
		color: var(--text-secondary);
		white-space: nowrap;
	}

	.backend-status.gpu {
		border-color: #2f7d4f;
		background: rgba(47, 125, 79, 0.08);
	}

	.backend-status.gpu .backend-dot {
		background: #36c46f;
	}

	.backend-status.cpu {
		border-color: #9a6b1f;
		background: rgba(154, 107, 31, 0.08);
	}

	.backend-status.cpu .backend-dot {
		background: #e0a23a;
	}

	.browse-btn {
		background: var(--neutral-800);
		color: var(--text-primary);
		border: 1px solid var(--border-subtle);
		border-radius: var(--radius-sm);
		padding: 0 1.5rem;
		font-family: var(--font-mono);
		font-size: 0.875rem;
		cursor: pointer;
		transition: all var(--duration-fast);
		white-space: nowrap;
	}

	.browse-btn:hover:not(:disabled) {
		background: var(--neutral-700);
		border-color: var(--neutral-600);
	}

	.browse-btn:disabled {
		opacity: 0.5;
		cursor: not-allowed;
	}

	.toggle-row {
		display: flex;
		align-items: flex-start;
		gap: var(--space-2);
		margin-top: var(--space-3);
		cursor: pointer;
	}

	.toggle-row input {
		margin-top: 0.2rem;
		flex: 0 0 auto;
	}

	.toggle-text {
		display: flex;
		flex-direction: column;
		gap: 0.15rem;
	}

	.toggle-text strong {
		color: var(--text-primary);
		font-size: 0.9rem;
	}

	.toggle-hint {
		font-size: 0.8rem;
		color: var(--text-secondary);
		line-height: 1.4;
	}

	.toggle-label {
		display: flex;
		align-items: center;
		gap: 0.4rem;
	}

	/* Little circled "i" that reveals detail/caveats on hover or focus, so the
       long explanation isn't pasted under every toggle. */
	.info {
		position: relative;
		display: inline-flex;
		outline: none;
	}

	.info-dot {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		width: 14px;
		height: 14px;
		border-radius: 50%;
		border: 1px solid var(--text-secondary);
		color: var(--text-secondary);
		font-size: 0.62rem;
		font-style: italic;
		font-weight: 700;
		line-height: 1;
		cursor: help;
		user-select: none;
	}

	.info:hover .info-dot,
	.info:focus .info-dot {
		border-color: var(--text-primary);
		color: var(--text-primary);
	}

	.info-pop {
		position: absolute;
		bottom: calc(100% + 8px);
		left: 0;
		z-index: 20;
		width: 320px;
		max-width: 60vw;
		padding: 0.6rem 0.7rem;
		background: var(--bg-elevated, #1e1e1e);
		border: 1px solid var(--border, rgba(255, 255, 255, 0.12));
		border-radius: 8px;
		box-shadow: 0 8px 24px rgba(0, 0, 0, 0.35);
		color: var(--text-secondary);
		font-size: 0.78rem;
		font-weight: 400;
		line-height: 1.45;
		opacity: 0;
		visibility: hidden;
		transform: translateY(4px);
		transition:
			opacity 0.12s ease,
			transform 0.12s ease,
			visibility 0.12s;
		pointer-events: none;
	}

	.info-pop strong {
		color: var(--text-primary);
		font-size: inherit;
	}

	.info:hover .info-pop,
	.info:focus .info-pop {
		opacity: 1;
		visibility: visible;
		transform: translateY(0);
	}

	.advanced-grid {
		display: grid;
		grid-template-columns: 1fr 1fr;
		gap: var(--space-4);
		margin-top: var(--space-2);
	}

	.input-group {
		display: flex;
		flex-direction: column;
		gap: var(--space-2);
	}

	.input-group.full-width {
		grid-column: 1 / -1;
	}

	.input-group label {
		font-size: 0.875rem;
		color: var(--text-secondary);
		font-family: var(--font-sans);
	}

	.input-group input {
		background: var(--bg-page);
		border: 1px solid var(--border-default);
		border-radius: var(--radius-sm);
		padding: 0.75rem 1rem;
		color: var(--text-primary);
		font-family: var(--font-mono);
		font-size: 0.875rem;
	}

	.input-group input:focus {
		outline: none;
		border-color: var(--accent-200);
	}

	/* Adaptive offload manages Context Size + GPU Layers, so they're locked
       (disabled) when it's on — show that clearly instead of looking editable. */
	.input-group input:disabled {
		opacity: 0.45;
		background: var(--bg-panel);
		color: var(--text-muted);
		cursor: not-allowed;
	}

	.success-message {
		position: fixed;
		bottom: var(--space-6);
		right: var(--space-6);
		background: var(--bg-panel);
		border: 1px solid var(--border-default);
		color: #4ade80;
		padding: var(--space-3) var(--space-4);
		border-radius: var(--radius-sm);
		box-shadow:
			0 4px 6px -1px rgba(0, 0, 0, 0.1),
			0 2px 4px -1px rgba(0, 0, 0, 0.06);
		font-size: 0.875rem;
		font-family: var(--font-sans);
		animation: fade-in var(--duration-fast) ease-out;
		z-index: 50;
		display: flex;
		align-items: center;
		gap: var(--space-2);
	}

	@keyframes fade-in {
		from {
			opacity: 0;
			transform: translateY(-4px);
		}
		to {
			opacity: 1;
			transform: translateY(0);
		}
	}
</style>
