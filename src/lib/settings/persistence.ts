import { invoke } from '@tauri-apps/api/core';
import { open } from '@tauri-apps/plugin-dialog';
import { inferencePayload, openharnPayload } from './model';
import type { OpenharnForm } from './types';

type BackendPref = 'auto' | 'cuda' | 'vulkan' | 'metal' | 'cpu';

export type SettingsPersistencePort = {
	get currentModelPath(): string;
	get contextSize(): number | null;
	get gpuLayers(): number | null;
	get threads(): number | null;
	get temperature(): number | null;
	get topP(): number | null;
	get maxTurns(): number | null;
	get extraArgs(): string[];
	get backendPreference(): BackendPref;
	get thinking(): boolean;
	get autoOffload(): boolean;
	get ohPort(): number | null;
	get ohBinPath(): string;
	get ohToolMode(): OpenharnForm['toolMode'];
	get ohStrict(): boolean;
	get ohPromptTools(): boolean;
	get ohCallOnly(): boolean;
	get ohNoThink(): boolean;
	get ohToolChoice(): string;
	get ohTemplateKwargs(): string;
	get ohMaxCalls(): number | null;
	get ohTotalMax(): number | null;
	get ohToolTimeout(): number | null;
	get ohBaseUrl(): string;
	get externalEnabled(): boolean;
	get externalBaseUrl(): string;
	get externalModel(): string;
	get externalApiKey(): string;
	set ohBinPath(value: string);
	set ohStrict(value: boolean);
	set ohCallOnly(value: boolean);
	set ohPromptTools(value: boolean);
	set isSaving(value: boolean);
	set saved(value: boolean);
	set ohSaving(value: boolean);
	set enableJupyterExecution(value: boolean);
	set extraArgs(value: string[]);
};

export function createSettingsPersistence(ctx: SettingsPersistencePort) {
	let saveTimeout: ReturnType<typeof setTimeout> | undefined;

	function markSaved() {
		ctx.saved = true;
		setTimeout(() => {
			ctx.saved = false;
		}, 3000);
	}

	async function saveModelPath() {
		if (!ctx.currentModelPath) return;
		ctx.isSaving = true;
		ctx.saved = false;
		try {
			await invoke('set_llama_model_path', { modelPath: ctx.currentModelPath });
			markSaved();
		} catch (error) {
			console.error('Failed to save model path:', error);
			alert('Failed to save model path: ' + error);
		} finally {
			ctx.isSaving = false;
		}
	}

	async function saveAdvancedConfig() {
		ctx.isSaving = true;
		ctx.saved = false;
		try {
			await invoke(
				'set_llama_advanced_config',
				inferencePayload({
					contextSize: ctx.contextSize,
					gpuLayers: ctx.gpuLayers,
					threads: ctx.threads,
					temperature: ctx.temperature,
					topP: ctx.topP,
					maxTurns: ctx.maxTurns,
					extraArgs: ctx.extraArgs,
					backendPreference: ctx.backendPreference,
					thinking: ctx.thinking,
					autoOffload: ctx.autoOffload
				})
			);
			markSaved();
		} catch (error) {
			console.error('Failed to save advanced config:', error);
			alert('Failed to save advanced config: ' + error);
		} finally {
			ctx.isSaving = false;
		}
	}

	function debounceSave() {
		if (saveTimeout) clearTimeout(saveTimeout);
		saveTimeout = setTimeout(saveAdvancedConfig, 500);
	}

	function addExtraArg() {
		ctx.extraArgs.push('');
		debounceSave();
	}

	function removeExtraArg(index: number) {
		ctx.extraArgs.splice(index, 1);
		debounceSave();
	}

	function toggleJupyterExecution() {
		ctx.enableJupyterExecution = !ctx.enableJupyterExecution;
		localStorage.setItem('myelin_jupyter_exec', ctx.enableJupyterExecution.toString());
	}

	async function pickOpenharnBin() {
		const picked = await open({ multiple: false, title: 'Choose openharn-myelin binary' });
		if (typeof picked === 'string') {
			ctx.ohBinPath = picked;
			await saveOpenharn();
		}
	}

	function changeToolMode() {
		if (ctx.ohToolMode !== 'prompt') {
			ctx.ohStrict = false;
			ctx.ohCallOnly = false;
			ctx.ohPromptTools = false;
		}
		void saveOpenharn();
	}

	async function saveOpenharn() {
		ctx.ohSaving = true;
		try {
			const settings: OpenharnForm = {
				port: ctx.ohPort,
				binPath: ctx.ohBinPath,
				toolMode: ctx.ohToolMode,
				strict: ctx.ohStrict,
				promptTools: ctx.ohPromptTools,
				callOnly: ctx.ohCallOnly,
				noThink: ctx.ohNoThink,
				toolChoice: ctx.ohToolChoice,
				templateKwargs: ctx.ohTemplateKwargs,
				maxCalls: ctx.ohMaxCalls,
				totalMax: ctx.ohTotalMax,
				toolTimeoutSecs: ctx.ohToolTimeout,
				baseUrl: ctx.ohBaseUrl,
				externalEnabled: ctx.externalEnabled,
				externalBaseUrl: ctx.externalBaseUrl,
				externalModel: ctx.externalModel,
				externalApiKey: ctx.externalApiKey
			};
			await invoke('set_openharn_settings', { settings: openharnPayload(settings) });
			markSaved();
		} catch (error) {
			console.error('Failed to save agent settings:', error);
			alert('Failed to save agent settings: ' + error);
		} finally {
			ctx.ohSaving = false;
		}
	}

	function dispose() {
		if (saveTimeout) clearTimeout(saveTimeout);
	}

	return {
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
