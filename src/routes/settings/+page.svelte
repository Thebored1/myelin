<script lang="ts">
    import { onMount, onDestroy } from 'svelte';
    import { invoke } from '@tauri-apps/api/core';
    import { listen } from '@tauri-apps/api/event';
    import { open } from '@tauri-apps/plugin-dialog';
    import { goto } from '$app/navigation';
    import type { AppSnapshot, IndexState, ProviderStatus } from '$lib/types';

    type BackendPref = 'auto' | 'gpu' | 'vulkan' | 'cpu';

    let currentModelPath = $state('');
    let contextSize = $state<number | null>(null);
    let gpuLayers = $state<number | null>(null);
    let threads = $state<number | null>(null);
    let temperature = $state<number | null>(null);
    let topP = $state<number | null>(null);
    let maxTurns = $state<number | null>(null);
    let thinking = $state(false);
    let autoOffload = $state(true);
    let deterministicTools = $state(true);
    let extraArgs = $state<string[]>([]);
    let activeWorkspacePath = $state('');
    let indexState = $state<IndexState | null>(null);
    let activeProvider = $state('');
    let backendPreference = $state<BackendPref>('auto');
    let downloadableBackends = $state<string[]>([]);
    let download = $state<{ backend: string; phase: string; percent: number; message: string } | null>(null);
    let activeBackend = $state<string | null>(null);
    let nvidiaDetected = $state(false);
    let gpuAvailable = $state(true);
    let gpus = $state<string[]>([]);
    let installedBackends = $state<string[]>([]);
    let backendFellBack = $state(false);
    let providerHealthy = $state(true);
    let providerDetail = $state('');

    const hasGpuBuild = () => installedBackends.some((b) => b === 'cuda' || b === 'vulkan' || b === 'metal');
    const backendLabel = (b: string) => (b === 'cuda' ? 'CUDA' : b === 'vulkan' ? 'Vulkan' : b === 'metal' ? 'Metal' : 'CPU');

    // A "GPU" (performance) choice is only distinct from "Vulkan" when there's a
    // discrete GPU: an NVIDIA card (CUDA), or more than one GPU (a real iGPU +
    // dGPU split). A single non-NVIDIA GPU is integrated → GPU adds nothing over
    // Vulkan, so it's disabled. (Robust: no fragile codename matching — lspci
    // reports APUs under inconsistent names like "Lucienne".)
    const hasDedicatedGpu = $derived(nvidiaDetected || gpus.length >= 2);

    let statusPoll: ReturnType<typeof setInterval> | undefined;
    onDestroy(() => { if (statusPoll) clearInterval(statusPoll); });

    // Heads-up when the chosen GPU path isn't available / installed — the app
    // falls back to CPU automatically, so it's never a hard error.
    const gpuIssue = $derived.by((): { level: 'warn'; message: string } | null => {
        if (backendPreference === 'cpu') return null;
        if (!gpuAvailable) {
            return { level: 'warn', message: `No GPU detected${gpus.length ? ` (${gpus.join(', ')})` : ''} — running on CPU.` };
        }
        const need = backendPreference === 'vulkan' ? 'vulkan' : nvidiaDetected ? 'cuda' : 'vulkan';
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
                ? (installed('vulkan') ? 'vulkan' : 'cpu')
                : nvidiaDetected && installed('cuda') ? 'cuda'
                : installed('vulkan') ? 'vulkan'
                : installed('metal') ? 'metal'
                : 'cpu';

        if (backendPreference !== 'cpu' && backendFellBack) {
            return { level: 'cpu', title: 'Running on CPU', detail: 'The GPU could not be used — check the GPU and driver.' };
        }
        if (target !== 'cpu' && activeBackend && activeBackend !== 'cpu') {
            return { level: 'gpu', title: `Running on ${activeBackend.toUpperCase()}`, detail: 'GPU acceleration active.' };
        }
        if (target === 'cpu') {
            const detail =
                backendPreference === 'cpu'
                    ? 'CPU mode — the most reliable option (works with every model).'
                    : hasGpuBuild() ? 'No GPU available on this machine.' : 'Install a GPU build below to accelerate.';
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

    // Web search + embeddings/RAG + model compatibility (Phase 5).
    let searxngUrl = $state('');
    let embedModelPath = $state('');
    type ProfileInfo = { name: string; architecture?: string; namePattern?: string; role?: string; verified: boolean; notes?: string; supportsTools?: boolean };
    let modelProfiles = $state<ProfileInfo[]>([]);

    async function saveSearxng() {
        await invoke('set_searxng_url', { url: searxngUrl.trim() || null });
    }
    async function pickEmbedModel() {
        const picked = await open({ multiple: false, filters: [{ name: 'GGUF model', extensions: ['gguf'] }] });
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
        nvidiaDetected = status.nvidiaDetected ?? false;
        gpuAvailable = status.gpuAvailable ?? true;
        gpus = status.gpus ?? [];
        installedBackends = status.installedBackends ?? [];
        providerHealthy = status.healthy ?? true;
        providerDetail = status.detail ?? '';
        return status;
    }

    onMount(async () => {
        try {
            await refreshSnapshot();
            const status = await loadProviderStatus();
            downloadableBackends = await invoke<string[]>('downloadable_backends');
            // Auto = let the app pick; GPU = dedicated/fastest; Vulkan = integrated;
            // CPU = most reliable. Anything unrecognised (or unset) means Auto.
            const sp = status.config?.backendPreference;
            backendPreference =
                sp === 'cpu' ? 'cpu' : sp === 'vulkan' ? 'vulkan' : sp === 'gpu' ? 'gpu' : 'auto';
            // No dedicated GPU → the explicit "GPU" choice is meaningless; fall to
            // Vulkan. (Auto needs no fixup — it adapts on its own.)
            if (!hasDedicatedGpu && backendPreference === 'gpu') backendPreference = 'vulkan';
            thinking = status.config?.thinking ?? false;
            autoOffload = status.config?.autoOffload ?? true;
            deterministicTools = status.config?.deterministicTools ?? true;
            searxngUrl = (await invoke<string | null>('get_searxng_url')) ?? '';
            embedModelPath = (await invoke<string | null>('get_embed_model_path')) ?? '';
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
                extraArgs = (status.config?.extraArgs ?? status.resolved.extraArgs ?? []);
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

            // Live-update the backend badge when a server actually starts.
            await listen<{ backend: string; gpuOffloaded: boolean; fellBackToCpu: boolean }>(
                'ai://llama_backend',
                (event) => {
                    activeBackend = event.payload.backend;
                    backendFellBack = event.payload.fellBackToCpu;
                }
            );

            // Backend download progress.
            await listen<{ backend: string; phase: string; percent: number; message: string }>(
                'backend://download',
                async (event) => {
                    download = event.payload;
                    if (event.payload.phase === 'done') {
                        await loadProviderStatus();
                        setTimeout(() => {
                            if (download?.phase === 'done') download = null;
                        }, 4000);
                    }
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

    async function downloadBackend(backend: string) {
        try {
            await invoke('download_llama_backend', { backend });
        } catch (e) {
            console.error('Backend download failed:', e);
        }
    }

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
            const extraArgsArray = extraArgs.filter(arg => arg.trim() !== '');
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
                Select a model to use for local AI features. It runs completely offline on your device and is saved in app settings, not inside the notes workspace. <strong>Only <code>.gguf</code> models are supported</strong> (llama.cpp format).
            </p>

            <div class="model-picker">
                <div class="path-display" class:empty={!currentModelPath}>
                    {currentModelPath || 'No model selected'}
                </div>
                <button class="browse-btn" onclick={selectModel} disabled={isSaving}>
                    Browse...
                </button>
            </div>
            

            <div class="compute-device">
                <span class="compute-label">Compute device</span>
                <div class="segmented" role="group" aria-label="Compute device">
                    {#each [{ value: 'auto', label: 'Auto' }, { value: 'gpu', label: 'GPU' }, { value: 'vulkan', label: 'Vulkan' }, { value: 'cpu', label: 'CPU' }] as opt}
                        {@const disabled = opt.value === 'gpu' && !hasDedicatedGpu}
                        <button
                            type="button"
                            class="segment"
                            class:active={backendPreference === opt.value}
                            {disabled}
                            title={disabled ? 'No dedicated GPU detected on this system' : ''}
                            onclick={() => selectBackend(opt.value as BackendPref)}
                        >
                            {opt.label}
                        </button>
                    {/each}
                </div>
                <p class="compute-hint">
                    {#if backendPreference === 'cpu'}
                        Most reliable: runs entirely on the CPU. Works with every model (some models give wrong output on the GPU), at the cost of speed.
                    {:else if backendPreference === 'vulkan'}
                        Power-saving: runs on the integrated GPU via Vulkan. The app still manages offload and falls back to CPU if needed.
                    {:else if backendPreference === 'gpu'}
                        Performance: uses the fastest available GPU (the dedicated GPU where present). Falls back automatically.
                    {:else}
                        Recommended: detects your hardware and picks the fastest backend — dedicated GPU (CUDA), integrated GPU (Vulkan), or CPU — and falls back on its own.
                    {/if}
                </p>

                {#if !hasDedicatedGpu && backendPreference !== 'cpu' && backendPreference !== 'auto'}
                    <div class="device-issue warn">
                        <span class="issue-icon">ℹ️</span>
                        <span>No dedicated GPU detected — GPU mode is unavailable. Using the integrated GPU via Vulkan, or switch to CPU for the most reliable output.</span>
                    </div>
                {/if}

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

            <div class="backend-status" class:gpu={computeStatus.level === 'gpu'} class:cpu={computeStatus.level === 'cpu'}>
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
                        {@const busy = download?.backend === b && download?.phase !== 'done' && download?.phase !== 'error'}
                        <div class="backend-item">
                            <span class="backend-name">{backendLabel(b)}</span>
                            {#if busy}
                                <div class="backend-progress">
                                    <div class="backend-bar"><div class="backend-bar-fill" style="width:{download?.percent ?? 0}%"></div></div>
                                    <span class="backend-progress-text">{download?.message ?? ''} ({Math.round(download?.percent ?? 0)}%)</span>
                                </div>
                            {:else if installed}
                                <span class="backend-installed">✓ Installed</span>
                            {:else}
                                <button class="browse-btn" onclick={() => downloadBackend(b)} disabled={!!download && download.phase !== 'done' && download.phase !== 'error'}>
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
                        CPU and Vulkan ship with the app. Download CUDA for the fastest speed on NVIDIA GPUs.
                    </p>
                </div>
            {/if}

            <br/>
            <h2>Web Search</h2>
            <p class="description">
                Privacy-first web search for the assistant. Set a SearXNG instance URL, or leave blank to use the no-key DuckDuckGo fallback.
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

            <br/>
            <h2>Embeddings — Document Search & RAG</h2>
            <p class="description">
                A small embedding model (e.g. nomic-embed-text v1.5, Q8_0) powers semantic search over your notes and lets the assistant search ingested documents (PDFs, books). It runs as a second local server.
            </p>
            <div class="model-picker">
                <div class="path-display" class:empty={!embedModelPath}>
                    {embedModelPath || 'No embedding model selected — semantic search uses a lexical fallback'}
                </div>
                <button class="browse-btn" onclick={pickEmbedModel}>Browse...</button>
                {#if embedModelPath}
                    <button class="browse-btn" onclick={clearEmbedModel}>Clear</button>
                {/if}
            </div>

            {#if modelProfiles.length > 0}
                <br/>
                <h2>Compatible Models</h2>
                <p class="description">
                    Models with a verified profile work out of the box (tool-calling and chat template tuned). Other models run on auto-detected defaults.
                </p>
                <div class="backends-list">
                    {#each modelProfiles as p}
                        <div class="backend-item">
                            <span class="backend-name">
                                {p.name}{#if p.role === 'embed'} <small>(embedding)</small>{/if}
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

            <br/>
            <h2>Advanced AI Configuration</h2>
            <p class="description">
                Fine-tune llama-server memory usage and CLI flags. Leave blank to use system defaults.
            </p>
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
                    <input type="number" id="ctx" bind:value={contextSize} oninput={debounceSave} placeholder="auto" disabled={autoOffload} />
                </div>
                <div class="input-group">
                    <label for="ngl">GPU Layers {autoOffload ? '(auto)' : ''}</label>
                    <input type="number" id="ngl" bind:value={gpuLayers} oninput={debounceSave} placeholder="auto" disabled={autoOffload} />
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

            <label class="toggle-row">
                <input type="checkbox" bind:checked={thinking} onchange={debounceSave} />
                <span class="toggle-text">
                    <strong>Model thinking / reasoning</strong>
                    <span class="toggle-hint">
                        {thinking
                            ? 'On — the model reasons before answering (slower, may be more accurate).'
                            : 'Off — faster, no hidden reasoning tokens. Works across models (Qwen, LFM, …).'}
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
                    <strong>Deterministic assists</strong>
                    <span class="toggle-hint">
                        {deterministicTools
                            ? 'On — rule-based shortcuts make small models reliable: strip headings/bold/bullets in code, exact word search, and a guard against wiping a note during an edit.'
                            : 'Off — the model handles every edit itself. Turn this off only if you run a stronger model and would rather it not be overridden.'}
                    </span>
                </span>
            </label>
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
