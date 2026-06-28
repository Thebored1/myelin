<script lang="ts">
	import { onMount } from 'svelte';
	import { invoke } from '@tauri-apps/api/core';
	import { emit, listen } from '@tauri-apps/api/event';
	import { getCurrentWebviewWindow } from '@tauri-apps/api/webviewWindow';

	let text = $state('');
	let workspacePath = $state<string | null>(null);
	let saved = $state(false);
	let inputEl: HTMLInputElement | undefined = $state();
	const win = getCurrentWebviewWindow();

	let tasks = $state<{ id: number; text: string; done: boolean }[]>([]);
	let activeFilter = $state<'all' | 'active' | 'done'>('all');

	let filteredTasks = $derived.by(() => {
		if (activeFilter === 'active') return tasks.filter(t => !t.done);
		if (activeFilter === 'done') return tasks.filter(t => t.done);
		return tasks;
	});

	function loadTasks() {
		if (!workspacePath) return;
		try {
			tasks = JSON.parse(localStorage.getItem(`tasks_${workspacePath}`) || '[]');
		} catch {
			tasks = [];
		}
	}

	function saveTasks() {
		if (!workspacePath) return;
		localStorage.setItem(`tasks_${workspacePath}`, JSON.stringify(tasks));
		emit('tasks://added');
	}

	async function loadWorkspace() {
		try {
			const snap = await invoke<{ workspacePath?: string }>('get_snapshot');
			workspacePath = snap.workspacePath ?? null;
			loadTasks();
		} catch {
			/* ignore */
		}
	}

	function addTask() {
		const t = text.trim();
		if (!t || !workspacePath) return;
		
		tasks.push({ id: Date.now(), text: t, done: false });
		saveTasks();
		
		text = '';
		saved = true;
		setTimeout(() => {
			saved = false;
		}, 1000);
	}

	function toggleTask(task: { id: number; text: string; done: boolean }) {
		task.done = !task.done;
		saveTasks();
	}

	function onKey(e: KeyboardEvent) {
		if (e.key === 'Enter') {
			e.preventDefault();
			addTask();
		} else if (e.key === 'Escape') {
			e.preventDefault();
			void win.hide();
		}
	}

	let shownAt = 0;

	onMount(() => {
		// Mark the document so the global stylesheet makes this window transparent
		// (only the floating card shows) without affecting the main window.
		document.documentElement.classList.add('quick-window');
		shownAt = Date.now();
		void loadWorkspace();
		setTimeout(() => inputEl?.focus(), 30);

		// Each time the global shortcut re-shows the window, clear + refocus.
		const un = listen('quick://focus', () => {
			text = '';
			saved = false;
			shownAt = Date.now();
			void loadWorkspace();
			setTimeout(() => inputEl?.focus(), 30);
		});
		// Dismiss when the window loses focus (click anywhere else). Guarded against
		// the brief blur that can fire right as the window is shown.
		const unfocus = win.onFocusChanged(({ payload: focused }) => {
			if (!focused && Date.now() - shownAt > 300) void win.hide();
		});
		return () => {
			document.documentElement.classList.remove('quick-window');
			void un.then((f) => f());
			void unfocus.then((f) => f());
		};
	});
</script>

<div class="quick-root">
	<div class="quick-card">
		<div class="quick-row">
			<svg class="quick-icon" viewBox="0 0 24 24" width="20" height="20" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
				<polyline points="9 11 12 14 22 4"></polyline>
				<path d="M21 12v7a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h11"></path>
			</svg>
			<!-- svelte-ignore a11y_autofocus -->
			<input
				bind:this={inputEl}
				bind:value={text}
				onkeydown={onKey}
				placeholder="Add a task…"
				spellcheck="false"
				autofocus
			/>
		</div>
		<div class="quick-hint">
			{#if saved}
				<span class="ok">Added ✓</span>
			{:else if !workspacePath}
				Open a workspace in Myelin first
			{:else}
				<kbd>Enter</kbd> to add · <kbd>Esc</kbd> to close
			{/if}
		</div>
	</div>

	{#if workspacePath && tasks.length > 0}
		<div class="quick-tasks-card">
			<div class="filters">
				<button class:active={activeFilter === 'all'} onclick={() => activeFilter = 'all'}>All</button>
				<button class:active={activeFilter === 'active'} onclick={() => activeFilter = 'active'}>Active</button>
				<button class:active={activeFilter === 'done'} onclick={() => activeFilter = 'done'}>Done</button>
			</div>
			<div class="task-list">
				{#each filteredTasks as task}
					<label class="task-item" class:done={task.done}>
						<input type="checkbox" checked={task.done} onchange={() => toggleTask(task)} />
						<span class="task-text">{task.text}</span>
					</label>
				{/each}
			</div>
		</div>
	{/if}
</div>

<style>
	/* Transparent window: only the floating card is visible (Spotlight-style).
	   Scoped to .quick-window so the main window's body background is untouched. */
	:global(html.quick-window),
	:global(html.quick-window body) {
		background: transparent !important;
	}
	.quick-root {
		height: 100vh;
		width: 100vw;
		display: flex;
		flex-direction: column;
		align-items: center;
		justify-content: flex-start;
		gap: 16px;
		/* Generous padding (in the transparent area) so the card's soft drop shadow
		   renders fully instead of being clipped at the window edges. */
		padding: 40px 44px 52px;
		box-sizing: border-box;
		background: transparent;
	}
	.quick-card,
	.quick-tasks-card {
		width: 100%;
		background: var(--bg-panel);
		border: 1px solid var(--border-default);
		border-radius: var(--radius-xl);
		padding: 16px 18px;
		box-shadow: 0 12px 40px var(--shadow-color-strong);
	}
	.quick-tasks-card {
		display: flex;
		flex-direction: column;
		gap: 12px;
		max-height: 400px;
	}
	.filters {
		display: flex;
		gap: 8px;
		border-bottom: 1px solid var(--border-subtle);
		padding-bottom: 8px;
	}
	.filters button {
		background: transparent;
		border: none;
		color: var(--text-secondary);
		cursor: pointer;
		font-size: 0.85rem;
		padding: 4px 8px;
		border-radius: var(--radius-sm);
	}
	.filters button:hover {
		background: var(--bg-hover);
	}
	.filters button.active {
		color: var(--text-primary);
		background: var(--bg-surface);
		border: 1px solid var(--border-default);
	}
	.task-list {
		overflow-y: auto;
		display: flex;
		flex-direction: column;
		gap: 8px;
	}
	.task-item {
		display: flex;
		align-items: flex-start;
		gap: 12px;
		cursor: pointer;
		padding: 4px 0;
	}
	.task-item input[type="checkbox"] {
		margin-top: 4px;
	}
	.task-text {
		color: var(--text-primary);
		font-size: 0.9rem;
		line-height: 1.4;
	}
	.task-item.done .task-text {
		color: var(--text-secondary);
		text-decoration: line-through;
	}
	.quick-row {
		display: flex;
		align-items: center;
		gap: 12px;
	}
	.quick-icon {
		color: var(--accent-200);
		flex-shrink: 0;
	}
	.quick-card input {
		flex: 1;
		min-width: 0;
		background: transparent;
		border: none;
		outline: none;
		color: var(--text-primary);
		font-size: 1.3rem;
	}
	.quick-hint {
		margin-top: 10px;
		font-size: 0.75rem;
		color: var(--text-secondary);
	}
	.quick-hint .ok {
		color: var(--success, #4caf50);
	}
	kbd {
		background: var(--bg-code);
		border: 1px solid var(--border-default);
		border-radius: var(--radius-sm);
		padding: 1px 5px;
		font-family: var(--font-mono);
		font-size: 0.7rem;
	}
</style>
