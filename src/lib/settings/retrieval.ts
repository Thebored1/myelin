import { invoke } from '@tauri-apps/api/core';
import { open } from '@tauri-apps/plugin-dialog';

type OcrSettings = {
	autoLowTextPages: boolean;
	executablePath?: string | null;
	language: string;
};

type RetrievalPort = {
	get searxngUrl(): string;
	get embedModelPath(): string;
	set embedModelPath(value: string);
	get rerankerModelPath(): string;
	set rerankerModelPath(value: string);
	set modelDownloadError(value: string);
	set downloadingModel(value: string);
	set ocrSaving(value: boolean);
	set ocrStatus(value: unknown);
};

export function createRetrievalController(port: RetrievalPort) {
	async function saveSearxng() {
		await invoke('set_searxng_url', { url: port.searxngUrl.trim() || null });
	}

	async function pickEmbedModel() {
		const picked = await open({
			multiple: false,
			filters: [{ name: 'GGUF model', extensions: ['gguf'] }]
		});
		if (typeof picked === 'string') {
			await invoke('set_embed_model_path', { path: picked });
			port.embedModelPath = picked;
		}
	}

	async function clearEmbedModel() {
		port.embedModelPath = '';
		await invoke('set_embed_model_path', { path: null });
	}

	async function pickRerankerModel() {
		const picked = await open({
			multiple: false,
			filters: [{ name: 'GGUF model', extensions: ['gguf'] }]
		});
		if (typeof picked === 'string') {
			await invoke('set_reranker_model_path', { path: picked });
			port.rerankerModelPath = picked;
		}
	}

	async function clearRerankerModel() {
		port.rerankerModelPath = '';
		await invoke('set_reranker_model_path', { path: null });
	}

	async function downloadBuiltInModel(id: string) {
		port.modelDownloadError = '';
		port.downloadingModel = id;
		try {
			const path = await invoke<string>('download_built_in_model', { id });
			if (id.startsWith('nomic-')) port.embedModelPath = path;
			else port.rerankerModelPath = path;
		} catch (error) {
			port.modelDownloadError = String(error);
		} finally {
			port.downloadingModel = '';
		}
	}

	async function saveOcrSettings(next: OcrSettings) {
		port.ocrSaving = true;
		try {
			port.ocrStatus = await invoke('set_ocr_settings', { settings: next });
		} catch (error) {
			port.modelDownloadError = String(error);
		} finally {
			port.ocrSaving = false;
		}
	}

	return {
		saveSearxng,
		pickEmbedModel,
		clearEmbedModel,
		pickRerankerModel,
		clearRerankerModel,
		downloadBuiltInModel,
		saveOcrSettings
	};
}
