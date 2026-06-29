<script lang="ts">
	import { onMount } from 'svelte';
	import { invoke } from '@tauri-apps/api/core';
	import { emit, listen } from '@tauri-apps/api/event';
	import { getCurrentWebviewWindow } from '@tauri-apps/api/webviewWindow';
	import { LogicalSize } from '@tauri-apps/api/dpi';

	let text = $state('');
	let workspacePath = $state<string | null>(null);
	let saved = $state(false);
	let inputEl: HTMLInputElement | undefined = $state();
	let rootEl: HTMLElement | undefined = $state();
	const win = getCurrentWebviewWindow();

	let draftDetails = $state('');
	let draftDueDate = $state('');
	let draftDueTime = $state('');
	let draftNotebook = $state('');
	let draftSubtasks = $state<TaskSubtask[]>([]);

	let notebooks = $state<string[]>([]);
	let expandedTaskId = $state<number | null>(null);

	interface TaskSubtask {
		id: number;
		text: string;
		done: boolean;
	}

	interface TaskItem {
		id: number;
		text: string;
		done: boolean;
		details?: string;
		dueDate?: string;
		dueTime?: string;
		notebook?: string;
		subtasks?: TaskSubtask[];
	}

	let tasks = $state<TaskItem[]>([]);
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
			notebooks = await invoke<string[]>('list_notebooks');
		} catch {
			/* ignore */
		}
	}

	function addTask() {
		const t = text.trim();
		if (!t || !workspacePath) return;
		
		tasks.push({ 
			id: Date.now(), 
			text: t, 
			done: false,
			details: draftDetails,
			dueDate: draftDueDate,
			dueTime: draftDueTime,
			notebook: draftNotebook,
			subtasks: [...draftSubtasks]
		});
		
		text = '';
		draftDetails = '';
		draftDueDate = '';
		draftDueTime = '';
		draftNotebook = '';
		draftSubtasks = [];
		
		saved = true;
		setTimeout(() => {
			saved = false;
		}, 1000);
	}

	$effect(() => {
		if (workspacePath) {
			localStorage.setItem(`tasks_${workspacePath}`, JSON.stringify(tasks));
			emit('tasks://added');
		}
	});

	function toggleTask(task: TaskItem) {
		task.done = !task.done;
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

	function handleKeydown(e: KeyboardEvent) {
		if (e.key === 'Tab') {
			const root = document.querySelector('.quick-app-root');
			if (!root) return;
			const focusableElements = root.querySelectorAll<HTMLElement>(
				'button, [href], input, select, textarea, [tabindex]:not([tabindex="-1"])'
			);
			
			const focusable = Array.from(focusableElements).filter(el => {
				return el.offsetWidth > 0 || el.offsetHeight > 0 || el === document.activeElement;
			});

			if (focusable.length === 0) return;

			const firstElement = focusable[0];
			const lastElement = focusable[focusable.length - 1];

			if (e.shiftKey) {
				if (document.activeElement === firstElement || document.activeElement === document.body || !document.activeElement) {
					e.preventDefault();
					if (document.activeElement !== firstElement) {
						firstElement.focus();
					}
				}
			} else {
				if (document.activeElement === lastElement) {
					e.preventDefault();
				}
			}
		}
	}

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
			draftDetails = '';
			draftDueDate = '';
			draftDueTime = '';
			draftNotebook = '';
			draftSubtasks = [];
			saved = false;
			expandedTaskId = null;
			shownAt = Date.now();
			void loadWorkspace();
			setTimeout(() => inputEl?.focus(), 30);
		});
		// Dismiss when the window loses focus (click anywhere else). Guarded against
		// the brief blur that can fire right as the window is shown.
		const unfocus = win.onFocusChanged(({ payload: focused }) => {
			if (!focused && Date.now() - shownAt > 300) void win.hide();
		});

		const ro = new ResizeObserver((entries) => {
			for (const entry of entries) {
				const rect = entry.target.getBoundingClientRect();
				// Use LogicalSize to resize the Tauri OS window to precisely match the new content height.
				// This ensures the window "expands as much as it wants" without clipping or internal scrolling.
				win.setSize(new LogicalSize(rect.width, rect.height)).catch(console.error);
			}
		});

		// Use a slight timeout to ensure the DOM is painted before observing
		setTimeout(() => {
			if (rootEl) ro.observe(rootEl);
		}, 0);

		return () => {
			ro.disconnect();
			document.documentElement.classList.remove('quick-window');
			void un.then((f) => f());
			void unfocus.then((f) => f());
		};
	});
</script>

<svelte:window onkeydown={handleKeydown} />

<div class="quick-app-root" bind:this={rootEl}>
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

	{#if workspacePath}
		{#if text.trim().length > 0}
			<div class="quick-tasks-container draft-card">
				<div class="task-expanded-details redesigned">
					<div class="field-row notebook-row">
						<select bind:value={draftNotebook} class="field-input select-new" onfocus={(e) => { try { e.currentTarget.showPicker(); } catch (err) {} }}>
							<option value="">Today</option>
							{#each notebooks as nb}
								<option value={nb}>{nb}</option>
							{/each}
						</select>
					</div>

					<div class="field-row">
						<svg class="field-icon" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2" fill="none"><path d="M4 6h16M4 12h16M4 18h16" /></svg>
						<textarea placeholder="Add details" bind:value={draftDetails} class="field-input textarea-new"></textarea>
					</div>

					<div class="field-row">
						<svg class="field-icon" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2" fill="none"><circle cx="12" cy="12" r="10"/><circle cx="12" cy="12" r="6"/><circle cx="12" cy="12" r="2"/></svg>
						<input 
							type="text" 
							placeholder="Add deadline" 
							bind:value={draftDueDate} 
							class="field-input date-time-new" 
							onfocus={(e) => e.currentTarget.type = 'date'} 
							onblur={(e) => { if (!e.currentTarget.value) e.currentTarget.type = 'text'; }} 
						/>
					</div>

					<div class="field-row">
						<svg class="field-icon" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2" fill="none"><circle cx="12" cy="12" r="10"/><path d="M12 6v6l4 2"/></svg>
						<input 
							type="text" 
							placeholder="Add date/time" 
							bind:value={draftDueTime} 
							class="field-input date-time-new" 
							onfocus={(e) => e.currentTarget.type = 'time'} 
							onblur={(e) => { if (!e.currentTarget.value) e.currentTarget.type = 'text'; }} 
						/>
					</div>

					<div class="subtasks-container">
						{#each draftSubtasks as subtask, i}
							<div class="subtask-row">
								<svg class="subtask-arrow" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2" fill="none"><path d="M6 4v6a2 2 0 0 0 2 2h10" /><path d="M15 9l3 3-3 3" /></svg>
								<button class="subtask-circle" class:done={subtask.done} onclick={() => subtask.done = !subtask.done} tabindex="-1"></button>
								<input type="text" bind:value={subtask.text} class="field-input subtask-input-new" class:done={subtask.done} />
								<button class="subtask-remove" tabindex="-1" onclick={() => draftSubtasks.splice(i, 1)}>&times;</button>
							</div>
						{/each}
						<div class="subtask-row">
							<svg class="subtask-arrow" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2" fill="none"><path d="M6 4v6a2 2 0 0 0 2 2h10" /><path d="M15 9l3 3-3 3" /></svg>
							<div class="subtask-circle empty"></div>
							<input type="text" placeholder="Enter title" class="field-input subtask-input-new" onkeydown={(e) => {
								if (e.key === 'Enter' && e.currentTarget.value.trim()) {
									e.preventDefault();
									draftSubtasks.push({ id: Date.now(), text: e.currentTarget.value.trim(), done: false });
									e.currentTarget.value = '';
								}
							}} />
						</div>
						<div class="subtask-add-hint">Add subtasks</div>
					</div>
				</div>
			</div>
		{:else if tasks.length > 0}
			<div class="quick-tasks-container">
				<div class="filters">
					<button class:active={activeFilter === 'all'} onclick={() => activeFilter = 'all'}>All</button>
					<button class:active={activeFilter === 'active'} onclick={() => activeFilter = 'active'}>Active</button>
					<button class:active={activeFilter === 'done'} onclick={() => activeFilter = 'done'}>Done</button>
				</div>
				<div class="task-list">
					{#each filteredTasks as task (task.id)}
						<div class="task-card" class:expanded={expandedTaskId === task.id}>
							<div class="task-item" class:done={task.done}>
								<button class="subtask-circle main-task-circle" class:done={task.done} onclick={() => task.done = !task.done} tabindex="-1"></button>
								<input class="task-text-input" type="text" bind:value={task.text} onfocus={() => expandedTaskId = task.id} />
								<button class="task-remove" tabindex="-1" onclick={(e) => { e.preventDefault(); tasks = tasks.filter(t => t.id !== task.id); }}>&times;</button>
							</div>
							{#if expandedTaskId === task.id}
								<div class="task-expanded-details redesigned">
									<div class="field-row notebook-row">
										<select bind:value={task.notebook} class="field-input select-new" onfocus={(e) => { try { e.currentTarget.showPicker(); } catch (err) {} }}>
											<option value="">Today</option>
											{#each notebooks as nb}
												<option value={nb}>{nb}</option>
											{/each}
										</select>
									</div>

									<div class="field-row">
										<svg class="field-icon" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2" fill="none"><path d="M4 6h16M4 12h16M4 18h16" /></svg>
										<textarea placeholder="Add details" bind:value={task.details} class="field-input textarea-new"></textarea>
									</div>

									<div class="field-row">
										<svg class="field-icon" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2" fill="none"><circle cx="12" cy="12" r="10"/><circle cx="12" cy="12" r="6"/><circle cx="12" cy="12" r="2"/></svg>
										<input 
											type="text" 
											placeholder="Add deadline" 
											bind:value={task.dueDate} 
											class="field-input date-time-new" 
											onfocus={(e) => e.currentTarget.type = 'date'} 
											onblur={(e) => { if (!e.currentTarget.value) e.currentTarget.type = 'text'; }} 
										/>
									</div>

									<div class="field-row">
										<svg class="field-icon" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2" fill="none"><circle cx="12" cy="12" r="10"/><path d="M12 6v6l4 2"/></svg>
										<input 
											type="text" 
											placeholder="Add date/time" 
											bind:value={task.dueTime} 
											class="field-input date-time-new" 
											onfocus={(e) => e.currentTarget.type = 'time'} 
											onblur={(e) => { if (!e.currentTarget.value) e.currentTarget.type = 'text'; }} 
										/>
									</div>

									<div class="subtasks-container">
										{#if task.subtasks}
											{#each task.subtasks as subtask, i}
												<div class="subtask-row">
													<svg class="subtask-arrow" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2" fill="none"><path d="M6 4v6a2 2 0 0 0 2 2h10" /><path d="M15 9l3 3-3 3" /></svg>
													<button class="subtask-circle" class:done={subtask.done} onclick={() => subtask.done = !subtask.done} tabindex="-1"></button>
													<input type="text" bind:value={subtask.text} class="field-input subtask-input-new" class:done={subtask.done} />
													<button class="subtask-remove" tabindex="-1" onclick={() => task.subtasks!.splice(i, 1)}>&times;</button>
												</div>
											{/each}
										{/if}
										<div class="subtask-row">
											<svg class="subtask-arrow" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2" fill="none"><path d="M6 4v6a2 2 0 0 0 2 2h10" /><path d="M15 9l3 3-3 3" /></svg>
											<div class="subtask-circle empty"></div>
											<input type="text" placeholder="Enter title" class="field-input subtask-input-new" onkeydown={(e) => {
												if (e.key === 'Enter' && e.currentTarget.value.trim()) {
													e.preventDefault();
													task.subtasks = task.subtasks || [];
													task.subtasks.push({ id: Date.now(), text: e.currentTarget.value.trim(), done: false });
													e.currentTarget.value = '';
												}
											}} />
										</div>
										<div class="subtask-add-hint">Add subtasks</div>
									</div>
								</div>
							{/if}
						</div>
					{/each}
				</div>
			</div>
		{/if}
	{/if}
</div>

<style>
	/* Fully transparent window: only the floating cards are visible. BOTH html and
	   body must be cleared — html keeps an opaque --bg-page otherwise, which is what
	   was painting the window solid dark. (Requires the window's transparent:true,
	   which only applies after a tauri dev restart.) */
	:global(html.quick-window),
	:global(html.quick-window body) {
		background: transparent !important;
		background-image: none !important;
	}
	:global(html.quick-window :focus-visible) {
		outline: 1px solid var(--text-primary) !important;
		outline-offset: 2px !important;
		border-radius: 2px;
	}
	:global(html.quick-window ::selection) {
		background: rgba(255, 255, 255, 0.2) !important;
	}
	.quick-app-root {
		width: 100vw;
		padding: 40px 44px 52px;
		box-sizing: border-box;
		background: transparent;
	}
	.quick-card {
		margin-bottom: 16px;
	}
	.quick-card,
	.quick-tasks-container {
		width: 100%;
		background: var(--bg-panel);
		border: 1px solid var(--border-default);
		border-radius: var(--radius-xl);
		padding: 16px 18px;
		box-shadow: 0 12px 40px var(--shadow-color-strong);
	}
	.quick-tasks-container {
		display: flex;
		flex-direction: column;
		gap: 12px;
	}
	.filters {
		display: flex;
		gap: 20px;
		padding-bottom: 12px;
		margin-bottom: 4px;
	}
	.filters button {
		background: transparent;
		border: none;
		color: var(--text-secondary);
		cursor: pointer;
		font-size: 0.95rem;
		padding: 4px 0;
		font-weight: 500;
		transition: color 0.1s;
	}
	.filters button:hover {
		color: var(--text-primary);
	}
	.filters button.active {
		color: var(--text-primary);
	}
	.task-list {
		display: flex;
		flex-direction: column;
		gap: 4px;
	}
	.task-card {
		display: flex;
		flex-direction: column;
		background: transparent;
		transition: background 0.1s;
		border-radius: var(--radius-sm);
	}
	.task-card.expanded {
		background: var(--hover-overlay);
	}
	.task-item {
		display: flex;
		align-items: flex-start;
		gap: 16px;
		cursor: pointer;
		padding: 10px 8px;
		position: relative;
	}
	.task-card:not(.expanded):hover {
		background: var(--hover-overlay);
	}
	.main-task-circle {
		width: 18px;
		height: 18px;
		margin-top: 1px;
	}
	.task-text-input {
		flex: 1;
		min-width: 0;
		font-size: 0.85rem;
		color: var(--text-primary);
		background: transparent;
		border: none;
		outline: none;
		line-height: 1.5;
		padding-right: 14px;
		font-family: inherit;
	}
	.task-item.done .task-text-input {
		text-decoration: line-through;
		color: var(--neutral-600);
	}
	.task-remove {
		position: absolute;
		right: 2px;
		top: 9px;
		background: transparent;
		border: none;
		color: var(--text-secondary);
		cursor: pointer;
		font-size: 1.2rem;
		line-height: 1;
		opacity: 0;
		transition: opacity 0.1s;
	}
	.task-card:hover .task-remove {
		opacity: 1;
	}
	.task-remove:hover {
		color: var(--danger);
	}
	
	.task-expanded-details.redesigned {
		display: flex;
		flex-direction: column;
		gap: 16px;
		padding: 16px 16px 24px 44px;
	}
	.draft-card .task-expanded-details.redesigned {
		padding: 12px 0 0 0;
	}
	.task-card.expanded .task-text-input {
		font-size: 1.4rem;
		font-weight: 500;
		padding-top: 4px;
		padding-bottom: 4px;
	}
	.field-row {
		display: flex;
		align-items: center;
		gap: 16px;
	}
	.field-row.notebook-row {
		margin-bottom: -4px;
	}
	.field-icon {
		width: 20px;
		height: 20px;
		flex-shrink: 0;
		color: var(--text-secondary);
	}
	.field-input {
		flex: 1;
		background: transparent;
		border: none;
		outline: none;
		color: var(--text-primary);
		font-size: 1rem;
		font-family: inherit;
		min-width: 0;
	}
	.field-input::placeholder {
		color: var(--text-secondary);
	}
	.select-new {
		appearance: none;
		-webkit-appearance: none;
		cursor: pointer;
		font-weight: 500;
		font-size: 0.95rem;
		color: var(--text-secondary);
		padding: 0;
		background-image: url('data:image/svg+xml;utf8,<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="%23888" stroke-width="2"><path d="M6 9l6 6 6-6"/></svg>');
		background-repeat: no-repeat;
		background-position: right center;
		background-size: 16px;
		padding-right: 24px;
		width: auto;
		flex: none;
	}
	.textarea-new {
		resize: none;
		min-height: 24px;
		line-height: 1.5;
		padding: 0;
		overflow: hidden;
	}
	.date-time-new {
		cursor: pointer;
		font-size: 0.95rem;
	}
	.date-time-new::-webkit-calendar-picker-indicator {
		display: none;
		-webkit-appearance: none;
	}
	.subtasks-container {
		display: flex;
		flex-direction: column;
		gap: 10px;
		margin-top: 4px;
	}
	.subtask-row {
		display: flex;
		align-items: center;
		gap: 12px;
		position: relative;
	}
	.subtask-arrow {
		width: 16px;
		height: 16px;
		color: var(--text-secondary);
		margin-left: 2px;
		flex-shrink: 0;
	}
	.subtask-circle {
		width: 16px;
		height: 16px;
		border-radius: 50%;
		border: 1.5px solid var(--text-secondary);
		background: transparent;
		flex-shrink: 0;
		cursor: pointer;
		padding: 0;
	}
	.subtask-circle.done {
		background: var(--text-secondary);
	}
	.subtask-circle.empty {
		border-style: dashed;
		cursor: default;
	}
	.subtask-input-new {
		font-size: 0.95rem;
	}
	.subtask-input-new.done {
		text-decoration: line-through;
		color: var(--neutral-600);
	}
	.subtask-remove {
		background: transparent;
		border: none;
		color: var(--text-secondary);
		cursor: pointer;
		opacity: 0;
		font-size: 1.2rem;
	}
	.subtask-row:hover .subtask-remove {
		opacity: 1;
	}
	.subtask-remove:hover {
		color: var(--danger, #ff4444);
	}
	.subtask-add-hint {
		margin-left: 56px;
		font-size: 0.85rem;
		color: var(--text-secondary);
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
