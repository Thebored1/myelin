import { invoke } from '@tauri-apps/api/core';
import { configMatchPositions } from './model';

export type AiConfigStatus = {
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

type AiConfigPort = {
	get aiConfig(): AiConfigStatus | null;
	set aiConfig(value: AiConfigStatus | null);
	get aiConfigBusy(): boolean;
	set aiConfigBusy(value: boolean);
	get aiConfigMessage(): string;
	set aiConfigMessage(value: string);
	get showAiConfig(): boolean;
	set showAiConfig(value: boolean);
	get aiConfigText(): string;
	set aiConfigText(value: string);
	get aiConfigSearch(): string;
	get aiConfigSearchInput(): HTMLInputElement | undefined;
	get aiConfigEditor(): HTMLTextAreaElement | undefined;
	get aiConfigSearchIndex(): number;
	set aiConfigSearchIndex(value: number);
};

export function createAiConfigController(port: AiConfigPort) {
	const configMatches = () => configMatchPositions(port.aiConfigText, port.aiConfigSearch);

	function gotoConfigMatch(direction = 1) {
		const positions = configMatches();
		if (!positions.length) return;
		port.aiConfigSearchIndex =
			(port.aiConfigSearchIndex + direction + positions.length) % positions.length;
		const start = positions[port.aiConfigSearchIndex];
		const end = start + port.aiConfigSearch.trim().length;
		port.aiConfigEditor?.focus();
		port.aiConfigEditor?.setSelectionRange(start, end);
		if (port.aiConfigEditor) {
			const lineHeight = parseFloat(getComputedStyle(port.aiConfigEditor).lineHeight) || 20;
			const line = port.aiConfigText.slice(0, start).split('\n').length - 1;
			port.aiConfigEditor.scrollTop = Math.max(
				0,
				line * lineHeight - port.aiConfigEditor.clientHeight / 2
			);
		}
	}

	function handleConfigSearchKeydown(event: KeyboardEvent) {
		if (event.key === 'Enter') {
			event.preventDefault();
			gotoConfigMatch(event.shiftKey ? -1 : 1);
		}
	}

	function configMatchCount() {
		return configMatches().length;
	}

	async function refreshAiConfig() {
		port.aiConfig = await invoke<AiConfigStatus>('get_ai_config_status');
	}

	async function validateAiConfig() {
		port.aiConfigBusy = true;
		port.aiConfigMessage = '';
		try {
			port.aiConfig = await invoke<AiConfigStatus>('validate_ai_config');
			port.aiConfigMessage = 'Configuration validated.';
		} catch (error) {
			port.aiConfigMessage = String(error);
			await refreshAiConfig();
		} finally {
			port.aiConfigBusy = false;
		}
	}

	async function applyAiConfig() {
		if (!port.aiConfig?.candidateHash) return;
		port.aiConfigBusy = true;
		port.aiConfigMessage = '';
		try {
			port.aiConfig = await invoke<AiConfigStatus>('apply_ai_config', {
				candidateHash: port.aiConfig.candidateHash
			});
			port.aiConfigMessage = 'Configuration applied.';
		} catch (error) {
			port.aiConfigMessage = String(error);
		} finally {
			port.aiConfigBusy = false;
		}
	}

	async function copyAiConfigPath() {
		if (port.aiConfig?.configPath) await navigator.clipboard?.writeText(port.aiConfig.configPath);
	}

	async function openAiConfig() {
		try {
			await invoke('open_ai_config_file');
			port.aiConfigText = await invoke<string>('read_ai_config');
			port.showAiConfig = true;
		} catch (error) {
			port.aiConfigMessage = String(error);
		}
	}

	async function saveAiConfigText() {
		port.aiConfigBusy = true;
		try {
			await invoke('save_ai_config', { contents: port.aiConfigText });
			await refreshAiConfig();
			port.showAiConfig = false;
			port.aiConfigMessage = 'Configuration saved. Validate it before applying.';
		} catch (error) {
			port.aiConfigMessage = String(error);
		} finally {
			port.aiConfigBusy = false;
		}
	}

	return {
		gotoConfigMatch,
		handleConfigSearchKeydown,
		configMatchCount,
		refreshAiConfig,
		validateAiConfig,
		applyAiConfig,
		copyAiConfigPath,
		openAiConfig,
		saveAiConfigText
	};
}
