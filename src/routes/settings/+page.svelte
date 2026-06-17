<script lang="ts">
    import { onMount } from 'svelte';
    import { invoke } from '@tauri-apps/api/core';
    import { listen } from '@tauri-apps/api/event';
    import { open } from '@tauri-apps/plugin-dialog';
    import { goto } from '$app/navigation';
    import type { AppSnapshot, IndexState, ProviderStatus } from '$lib/types';

    let currentModelPath = $state('');
    let currentExecutablePath = $state('');
    let contextSize = $state<number | null>(null);
    let gpuLayers = $state<number | null>(null);
    let threads = $state<number | null>(null);
    let temperature = $state<number | null>(null);
    let topP = $state<number | null>(null);
    let maxTurns = $state<number | null>(null);
    let extraArgs = $state<string[]>([]);
    let activeWorkspacePath = $state('');
    let indexState = $state<IndexState | null>(null);
    let activeProvider = $state('');
    let backendPreference = $state<'auto' | 'gpu' | 'cpu'>('auto');
    let activeBackend = $state<string | null>(null);
    let nvidiaDetected = $state(false);
    let gpuAvailable = $state(true);
    let gpus = $state<string[]>([]);
    let installedBackends = $state<string[]>([]);
    let backendFellBack = $state(false);
    let providerHealthy = $state(true);
    let providerDetail = $state('');

    // Proactive validation of the chosen compute device against this machine.
    const gpuIssue = $derived.by((): { level: 'error' | 'warn'; message: string } | null => {
        if (backendPreference === 'cpu') return null;

        if (!gpuAvailable) {
            // Only a hard error when the user explicitly forces GPU.
            if (backendPreference === 'gpu') {
                return {
                    level: 'error',
                    message:
                        'No GPU was detected on this system' +
                        (gpus.length ? ` (${gpus.join(', ')})` : '') +
                        '. GPU mode is unavailable here — switch to Auto or CPU.'
                };
            }
            return null; // Auto on a GPU-less machine correctly uses CPU.
        }

        const hasGpuBuild =
            installedBackends.includes('cuda') ||
            installedBackends.includes('vulkan') ||
            installedBackends.includes('metal');
        if (!hasGpuBuild) {
            const which = nvidiaDetected ? 'CUDA (bin/cuda/)' : 'Vulkan (bin/vulkan/)';
            return {
                level: 'warn',
                message: `A GPU was detected (${gpus.join(', ') || 'unknown'}), but no GPU build is installed. Add a ${which} build — see docs/llama-backends.md — otherwise the app falls back to CPU.`
            };
        }
        return null;
    });
    let isSaving = $state(false);
    let isRebuilding = $state(false);
    let saved = $state(false);
    
    let enableJupyterExecution = $state(false);

    async function refreshSnapshot() {
        const snapshot = await invoke<AppSnapshot>('get_snapshot');
        activeWorkspacePath = snapshot.workspacePath || '';
        indexState = snapshot.indexState ?? null;
    }

    onMount(async () => {
        try {
            await refreshSnapshot();
            const status = await invoke<ProviderStatus>('get_provider_status');
            activeProvider = status.activeProvider || '';
            activeBackend = status.activeBackend ?? status.resolved?.backend ?? null;
            nvidiaDetected = status.nvidiaDetected ?? false;
            gpuAvailable = status.gpuAvailable ?? true;
            gpus = status.gpus ?? [];
            installedBackends = status.installedBackends ?? [];
            providerHealthy = status.healthy ?? true;
            providerDetail = status.detail ?? '';
            backendPreference = (status.config?.backendPreference as 'auto' | 'gpu' | 'cpu') ?? 'auto';
            if (status.resolved) {
                currentModelPath = status.config?.modelPath || status.resolved.modelPath || '';
                currentExecutablePath = status.config?.executablePath || status.resolved.executablePath || '';
                contextSize = status.config?.contextSize ?? status.resolved.contextSize ?? null;
                gpuLayers = status.config?.gpuLayers ?? status.resolved.gpuLayers ?? null;
                threads = status.config?.threads ?? status.resolved.threads ?? null;
                temperature = status.config?.temperature ?? status.resolved.temperature ?? null;
                topP = status.config?.topP ?? status.resolved.topP ?? null;
                maxTurns = status.config?.maxTurns ?? null;
                extraArgs = (status.config?.extraArgs ?? status.resolved.extraArgs ?? []);
            } else if (status.config) {
                currentModelPath = status.config.modelPath || '';
                currentExecutablePath = status.config.executablePath || '';
                contextSize = status.config.contextSize ?? null;
                gpuLayers = status.config.gpuLayers ?? null;
                threads = status.config.threads ?? null;
                temperature = status.config.temperature ?? null;
                topP = status.config.topP ?? null;
                maxTurns = status.config.maxTurns ?? null;
                extraArgs = status.config.extraArgs ?? [];
            }
            
            enableJupyterExecution = localStorage.getItem('myelin_jupyter_exec') === 'true';

            // Live-update the backend badge when a server actually starts.
            await listen<{ backend: string; gpuOffloaded: boolean; fellBackToCpu: boolean }>(
                'ai://llama_backend',
                (event) => {
                    activeBackend = event.payload.backend;
                    backendFellBack = event.payload.fellBackToCpu;
                }
            );
        } catch (e) {
            console.error('Failed to load provider status:', e);
        }
    });

    async function changeWorkspace() {
        const picked = await open({ directory: true, multiple: false, title: 'Choose your markdown workspace' });
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
                filters: [{
                    name: 'GGUF Model',
                    extensions: ['gguf']
                }]
            });
            
            if (selected && !Array.isArray(selected)) {
                currentModelPath = selected;
                await saveModelPath();
            }
        } catch (error) {
            console.error('Failed to open file dialog:', error);
        }
    }

    async function selectExecutable() {
        try {
            const selected = await open({
                multiple: false,
                filters: [{
                    name: 'Executable',
                    extensions: ['exe', '']
                }]
            });
            
            if (selected && !Array.isArray(selected)) {
                currentExecutablePath = selected;
                await saveExecutablePath();
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

    async function saveExecutablePath() {
        if (!currentExecutablePath) return;
        
        isSaving = true;
        saved = false;
        try {
            await invoke('set_llama_executable_path', { executablePath: currentExecutablePath });
            saved = true;
            setTimeout(() => {
                saved = false;
            }, 3000);
        } catch (error) {
            console.error('Failed to save executable path:', error);
            alert('Failed to save executable path: ' + error);
        } finally {
            isSaving = false;
        }
    }

    async function saveAdvancedConfig() {
        isSaving = true;
        saved = false;
        try {
            const extraArgsArray = extraArgs.filter(arg => arg.trim() !== '');
            await invoke('set_llama_advanced_config', {
                contextSize: contextSize,
                gpuLayers: gpuLayers,
                threads: threads,
                temperature: temperature,
                topP: topP,
                extraArgs: extraArgsArray.length > 0 ? extraArgsArray : null,
                backendPreference: backendPreference,
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
</script>

<div class="settings-container">
    <header class="settings-header">
        <button class="back-btn" onclick={() => goto('/')}>
            <svg viewBox="0 0 24 24" width="20" height="20" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
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
                    <span class="info-value">{indexState ? `${indexState.backend}:${indexState.noteCount} notes` : '—'}</span>
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
            <h2>Local AI Model Configuration</h2>
            <p class="description">
                Select a <code>.gguf</code> model to use for local AI features. This model will run completely offline on your device and is saved in app settings, not inside the notes workspace.
            </p>

            <div class="model-picker">
                <div class="path-display" class:empty={!currentModelPath}>
                    {currentModelPath || 'No model selected'}
                </div>
                <button class="browse-btn" onclick={selectModel} disabled={isSaving}>
                    Browse...
                </button>
            </div>
            
            <br/>
            <h2>Llama-Server Executable</h2>
            <p class="description">
                Select the <code>llama-server</code> executable file. This is the server engine that will run your model.
            </p>

            <div class="model-picker">
                <div class="path-display" class:empty={!currentExecutablePath}>
                    {currentExecutablePath || 'No executable selected'}
                </div>
                <button class="browse-btn" onclick={selectExecutable} disabled={isSaving}>
                    Browse...
                </button>
            </div>

            <div class="compute-device">
                <span class="compute-label">Compute device</span>
                <div class="segmented" role="group" aria-label="Compute device">
                    {#each [['auto', 'Auto'], ['gpu', 'GPU'], ['cpu', 'CPU']] as [value, label]}
                        {@const disabled = value === 'gpu' && !gpuAvailable}
                        <button
                            type="button"
                            class="segment"
                            class:active={backendPreference === value}
                            {disabled}
                            title={disabled ? 'No GPU detected on this system' : ''}
                            onclick={() => { backendPreference = value as 'auto' | 'gpu' | 'cpu'; debounceSave(); }}
                        >
                            {label}
                        </button>
                    {/each}
                </div>
                <p class="compute-hint">
                    {#if backendPreference === 'auto'}
                        Use the GPU when available, otherwise the CPU. {gpus.length ? `Detected: ${gpus.join(', ')}.` : 'No GPU detected.'}
                    {:else if backendPreference === 'gpu'}
                        Force GPU acceleration. Falls back to CPU with a warning if no GPU build is available.
                    {:else}
                        Force CPU only. Slower, but works everywhere. Takes effect after the AI server restarts.
                    {/if}
                </p>

                {#if gpuIssue}
                    <div class="device-issue" class:error={gpuIssue.level === 'error'} class:warn={gpuIssue.level === 'warn'}>
                        <span class="issue-icon">{gpuIssue.level === 'error' ? '⛔' : '⚠️'}</span>
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

            {#if activeBackend}
                <div class="backend-status" class:gpu={activeBackend !== 'cpu' && !backendFellBack} class:cpu={activeBackend === 'cpu' || backendFellBack}>
                    <span class="backend-dot"></span>
                    {#if backendFellBack}
                        Running on <strong>CPU</strong> — a GPU was requested but no device was used.
                        Install a GPU build (e.g. <code>cuda</code>) under the binary folder for full speed.
                    {:else if activeBackend === 'cpu'}
                        Running on <strong>CPU</strong>.{nvidiaDetected ? ' An NVIDIA GPU was detected — add a cuda/ build beside your llama-server binary for a big speedup.' : ''}
                    {:else}
                        GPU acceleration active: <strong>{activeBackend.toUpperCase()}</strong>.
                    {/if}
                </div>
            {/if}

            <br/>
            <h2>Advanced AI Configuration</h2>
            <p class="description">
                Fine-tune llama-server memory usage and CLI flags. Leave blank to use system defaults.
            </p>
            <div class="advanced-grid">
                <div class="input-group">
                    <label for="ctx">Context Size</label>
                    <input type="number" id="ctx" bind:value={contextSize} oninput={debounceSave} placeholder="4096" />
                </div>
                <div class="input-group">
                    <label for="ngl">GPU Layers</label>
                    <input type="number" id="ngl" bind:value={gpuLayers} oninput={debounceSave} placeholder="999" />
                </div>
                <div class="input-group">
                    <label for="threads">CPU Threads</label>
                    <input type="number" id="threads" bind:value={threads} oninput={debounceSave} placeholder="Auto" />
                </div>
                <div class="input-group">
                    <label for="temp">Temperature</label>
                    <input type="number" step="0.1" id="temp" bind:value={temperature} oninput={debounceSave} placeholder="0.2" />
                </div>
                <div class="input-group">
                    <label for="top_p">Top P</label>
                    <input type="number" step="0.05" id="top_p" bind:value={topP} oninput={debounceSave} placeholder="0.95" />
                </div>
                <div class="input-group">
                    <label for="max_turns">Max Tool Turns</label>
                    <input type="number" min="1" max="12" step="1" id="max_turns" bind:value={maxTurns} oninput={debounceSave} placeholder="4" />
                </div>
            </div>
            <div class="input-group full-width" style="margin-top: 1rem;">
                <label>
                    Extra Arguments
                    <div style="font-size: 0.8em; color: var(--text-error); margin-top: 4px;">
                        <strong>CRITICAL NOTE:</strong> Because of how system processes work, you cannot put spaces in a single box! If you wanted to add <code>--threads 8</code>, you must put <code>--threads</code> in one box, click add again, and put <code>8</code> in the next box!
                    </div>
                </label>
                {#each extraArgs as arg, i}
                    <div style="display: flex; gap: var(--space-2); margin-bottom: var(--space-2);">
                        <input type="text" bind:value={extraArgs[i]} oninput={debounceSave} placeholder="--flash-attn" style="flex: 1;" />
                        <button class="browse-btn" onclick={() => removeExtraArg(i)} title="Remove argument" style="padding: 0 1rem; color: #f87171; border-color: rgba(248, 113, 113, 0.3);">
                            Remove
                        </button>
                    </div>
                {/each}
                <button class="browse-btn" onclick={addExtraArg} style="align-self: flex-start; margin-top: 4px;">
                    + Add Argument
                </button>
            </div>
        </section>

        <section class="settings-section">
            <h2>Features</h2>
            <div class="feature-toggle" style="display: flex; justify-content: space-between; align-items: center; margin-top: 1rem;">
                <div>
                    <h3 style="margin: 0; font-size: 1rem;">Jupyter Code Execution</h3>
                    <p class="description" style="margin-top: 4px;">Allow execution of Python code cells within `.ipynb` notebooks using your local Python installation.</p>
                </div>
                <button class="browse-btn" onclick={toggleJupyterExecution}>
                    {enableJupyterExecution ? 'Enabled' : 'Disabled'}
                </button>
            </div>
        </section>
    </div>
</div>

{#if saved}
    <div class="success-message">
        <svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
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
        transition: background 0.12s, color 0.12s;
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
        align-items: center;
        gap: var(--space-2);
        margin-top: var(--space-3);
        padding: 0.6rem 0.9rem;
        border-radius: var(--radius-sm);
        border: 1px solid var(--border-default);
        font-size: 0.85rem;
        line-height: 1.4;
        color: var(--text-secondary);
    }

    .backend-status code {
        font-family: var(--font-mono);
        font-size: 0.8rem;
    }

    .backend-dot {
        flex: 0 0 auto;
        width: 8px;
        height: 8px;
        border-radius: 50%;
        background: var(--text-secondary);
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

    .success-message {
        position: fixed;
        bottom: var(--space-6);
        right: var(--space-6);
        background: var(--bg-panel);
        border: 1px solid var(--border-default);
        color: #4ade80;
        padding: var(--space-3) var(--space-4);
        border-radius: var(--radius-sm);
        box-shadow: 0 4px 6px -1px rgba(0, 0, 0, 0.1), 0 2px 4px -1px rgba(0, 0, 0, 0.06);
        font-size: 0.875rem;
        font-family: var(--font-sans);
        animation: fade-in var(--duration-fast) ease-out;
        z-index: 50;
        display: flex;
        align-items: center;
        gap: var(--space-2);
    }

    @keyframes fade-in {
        from { opacity: 0; transform: translateY(-4px); }
        to { opacity: 1; transform: translateY(0); }
    }
</style>
