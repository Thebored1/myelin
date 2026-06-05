<script lang="ts">
	import { invoke } from '@tauri-apps/api/core';
	import { listen } from '@tauri-apps/api/event';
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { open } from '@tauri-apps/plugin-dialog';
	import type {
		AppSnapshot,
		NoteDocument,
		NoteSummary,
		ProviderStatus,
		SearchResponse
	} from '$lib/types';
	import { onMount } from 'svelte';

	let app = $state<AppSnapshot | null>(null);
	let provider = $state<ProviderStatus | null>(null);
	let query = $state('');
	let sortMode = $state<'updated' | 'created' | 'title' | 'custom'>('updated');
	let viewMode = $state<'list' | 'grid'>('list');
	let filterFolder = $state('all');
	let filterTag = $state('all');
	let isBusy = $state(false);
	let message = $state('Booting local-first workspace...');
	let searchResults = $state<SearchResponse | null>(null);
	let pendingCreateCount = 0;
	let createLoopRunning = false;
	let sidebarOpen = $state(false);
	let visibleNotes = $derived.by(() => {
		const baseNotes = (
			query && searchResults
				? searchResults.results.map((result) => result.note)
				: (app?.notes ?? [])
		) as NoteSummary[];
		const filtered = baseNotes.filter((note) => {
			const folderMatch = filterFolder === 'all' || note.folder === filterFolder;
			const tagMatch = filterTag === 'all' || note.tags.includes(filterTag);
			return folderMatch && tagMatch;
		});
		const sorted = [...filtered];
		if (sortMode === 'title') {
			sorted.sort((left, right) => left.title.localeCompare(right.title));
		} else if (sortMode === 'created') {
			sorted.sort((left, right) => right.createdAt.localeCompare(left.createdAt));
		} else if (sortMode === 'custom') {
			const orderMap = new Map((app?.customNoteOrder ?? []).map((id, index) => [id, index]));
			sorted.sort((left, right) => {
				const leftIndex = orderMap.get(left.id) ?? Number.MAX_SAFE_INTEGER;
				const rightIndex = orderMap.get(right.id) ?? Number.MAX_SAFE_INTEGER;
				return leftIndex - rightIndex || right.updatedAt.localeCompare(left.updatedAt);
			});
		} else {
			sorted.sort((left, right) => right.updatedAt.localeCompare(left.updatedAt));
		}
		return sorted;
	});

	async function refreshApp() {
		app = await invoke<AppSnapshot>('get_snapshot');
		provider = await invoke<ProviderStatus>('get_provider_status');
		if (query.trim()) {
			searchResults = await invoke<SearchResponse>('search_notes', { query });
		} else {
			searchResults = null;
		}
	}

	function folderFromRelativePath(relativePath: string) {
		const segments = relativePath.split('/').filter(Boolean);
		return segments.length > 1 ? segments.slice(0, -1).join('/') : 'Root';
	}

	function excerptFromBody(body: string) {
		const flat = body.trim().replace(/\s+/g, ' ');
		return flat.length > 180 ? `${flat.slice(0, 180)}...` : flat;
	}

	function upsertNoteIntoLibrary(note: NoteDocument) {
		if (!app) return;

		const summary: NoteSummary = {
			id: note.id,
			title: note.title,
			tags: note.tags,
			folder: folderFromRelativePath(note.relativePath),
			excerpt: excerptFromBody(note.body),
			relativePath: note.relativePath,
			createdAt: note.createdAt,
			updatedAt: note.updatedAt
		};

		const existingIndex = app.notes.findIndex((entry) => entry.id === note.id);
		if (existingIndex >= 0) {
			app.notes[existingIndex] = summary;
		} else {
			app.notes = [summary, ...app.notes];
		}

		if (!app.customNoteOrder.includes(note.id)) {
			app.customNoteOrder = [...app.customNoteOrder, note.id];
		}

		if (!app.libraryFacets.folders.includes(summary.folder)) {
			app.libraryFacets.folders = [...app.libraryFacets.folders, summary.folder].sort();
		}

		const mergedTags = new Set([...app.libraryFacets.tags, ...summary.tags]);
		app.libraryFacets.tags = [...mergedTags].sort();
	}

	async function pickWorkspace() {
		const picked = await open({
			directory: true,
			multiple: false,
			title: 'Choose your markdown workspace'
		});

		if (typeof picked === 'string') {
			isBusy = true;
			try {
				app = await invoke<AppSnapshot>('set_workspace', { workspacePath: picked });
				message = 'Workspace connected and indexed.';
			} finally {
				isBusy = false;
			}
		}
	}

	async function createNote() {
		pendingCreateCount += 1;
		if (createLoopRunning) return;

		createLoopRunning = true;
		isBusy = true;
		let createdCount = 0;

		try {
			while (pendingCreateCount > 0) {
				pendingCreateCount -= 1;
				const note = await invoke<NoteDocument>('create_note', { title: 'New note' });
				upsertNoteIntoLibrary(note);
				createdCount += 1;
			}

			await refreshApp();
			message = createdCount === 1 ? '1 note created.' : `${createdCount} notes created.`;
		} finally {
			isBusy = false;
			createLoopRunning = false;
		}
	}

	async function runSearch() {
		searchResults = await invoke<SearchResponse>('search_notes', { query });
	}

	async function rebuild() {
		isBusy = true;
		try {
			app = await invoke<AppSnapshot>('rebuild_index');
			message = 'Manual reindex finished.';
		} finally {
			isBusy = false;
		}
	}

	function formatDate(value: string) {
		return new Intl.DateTimeFormat(undefined, {
			month: 'short',
			day: 'numeric',
			hour: 'numeric',
			minute: '2-digit'
		}).format(new Date(value));
	}

	async function openNote(noteId: string) {
		await goto(resolve(`/notes/${encodeURIComponent(noteId)}`));
	}

	onMount(() => {
		if (window.innerWidth > 1100) {
			sidebarOpen = true;
		}

		let unlistenChanged = () => {};
		let unlistenStatus = () => {};

		void (async () => {
			app = await invoke<AppSnapshot>('bootstrap');
			await refreshApp();
			unlistenChanged = await listen('index://changed', () => {
				message = 'Filesystem change detected. Reindexing...';
			});
			unlistenStatus = await listen<string>('index://status', (event) => {
				message = event.payload === 'started' ? 'Indexing workspace...' : 'Workspace ready.';
				if (event.payload === 'completed') {
					void refreshApp();
				}
			});
		})();

		return () => {
			unlistenChanged();
			unlistenStatus();
		};
	});
</script>

<svelte:head>
	<title>myelin</title>
</svelte:head>

<div class="shell">
	{#if sidebarOpen}
		<!-- svelte-ignore a11y_click_events_have_key_events -->
		<!-- svelte-ignore a11y_no_static_element_interactions -->
		<div class="sidebar-backdrop" onclick={() => sidebarOpen = false}></div>
	{/if}
	<aside class="rail" class:open={sidebarOpen}>
		<div>
			<p class="eyebrow">Cross-platform local notes</p>
			<h1>myelin</h1>
			<p class="copy">
				Browse the library here. Opening a note now takes you to a dedicated editor screen.
			</p>
		</div>

		<div class="stack">
			<button class="primary" onclick={pickWorkspace} disabled={isBusy}>Choose workspace</button>
			<button class="secondary" onclick={createNote} disabled={isBusy || !app?.workspacePath}>
				New note
			</button>
			<button class="secondary" onclick={rebuild} disabled={isBusy || !app?.workspacePath}>
				Rebuild index
			</button>
		</div>

		<div class="panel">
			<div class="panel-header">
				<span>Workspace</span>
				<strong>{app?.workspacePath ?? 'Not selected'}</strong>
			</div>
			<div class="panel-header">
				<span>Index</span>
				<strong>{app?.indexState.backend ?? 'lancedb'}</strong>
			</div>
			<div class="panel-header">
				<span>Provider</span>
				<strong>{provider?.activeProvider ?? 'loading'}</strong>
			</div>
		</div>
	</aside>

	<section class="content">
		<header class="topbar">
			<div style="display: flex; gap: var(--space-4); align-items: flex-start;">
				<button class="secondary sidebar-toggle-btn" onclick={() => sidebarOpen = !sidebarOpen} aria-label="Toggle sidebar">
					<svg viewBox="0 0 24 24" width="20" height="20" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
						<line x1="3" y1="12" x2="21" y2="12"></line>
						<line x1="3" y1="6" x2="21" y2="6"></line>
						<line x1="3" y1="18" x2="21" y2="18"></line>
					</svg>
				</button>
				<div>
					<p class="status">{message}</p>
					<h2>
						{app?.workspacePath
							? `${app.notes.length} notes indexed`
							: 'Connect a markdown workspace'}
					</h2>
				</div>
			</div>

			<label class="search">
				<input
					bind:value={query}
					oninput={runSearch}
					placeholder="Search titles, tags, or meaning..."
				/>
			</label>
		</header>

		<section class="notes">
			<div class="notes-header">
				<h3>Notes Library</h3>
				<span>{searchResults?.results.length ?? app?.notes.length ?? 0}</span>
			</div>

			<div class="library-controls">
				<select bind:value={sortMode}>
					<option value="updated">Last edited</option>
					<option value="created">Created</option>
					<option value="title">Title</option>
					<option value="custom">Custom order</option>
				</select>
				<select bind:value={filterFolder}>
					<option value="all">All folders</option>
					{#each app?.libraryFacets.folders ?? [] as folder (folder)}
						<option value={folder}>{folder}</option>
					{/each}
				</select>
				<select bind:value={filterTag}>
					<option value="all">All tags</option>
					{#each app?.libraryFacets.tags ?? [] as tag (tag)}
						<option value={tag}>{tag}</option>
					{/each}
				</select>
			</div>

			<div class="library-actions">
				<div class="view-toggle">
					<button
						class:active-toggle={viewMode === 'list'}
						class="secondary"
						onclick={() => (viewMode = 'list')}
						type="button"
					>
						List
					</button>
					<button
						class:active-toggle={viewMode === 'grid'}
						class="secondary"
						onclick={() => (viewMode = 'grid')}
						type="button"
					>
						Grid
					</button>
				</div>
				<button class="secondary" onclick={createNote} disabled={isBusy || !app?.workspacePath}>
					Create note
				</button>
			</div>

			<div class:notes-grid={viewMode === 'grid'} class="notes-list">
				{#each visibleNotes as note (note.id)}
					<button class="note-card" onclick={() => openNote(note.id)}>
						<strong>{note.title}</strong>
						<p>{note.excerpt || 'Empty note'}</p>
						<div class="meta">
							<span>{note.folder}</span>
							<span>{formatDate(note.updatedAt)}</span>
							<span>{note.tags.join(' · ')}</span>
						</div>
					</button>
				{/each}
			</div>
		</section>
	</section>
</div>

<style>
	.shell {
		display: flex;
		min-height: 100vh;
		position: relative;
		overflow: hidden;
		animation: fade-in var(--duration-page) var(--ease-out);
	}

	.sidebar-toggle-btn {
		display: none;
		align-items: center;
		justify-content: center;
		padding: 0.5rem;
		height: fit-content;
	}

	.sidebar-backdrop {
		display: none;
	}

	.rail {
		padding: var(--space-8) var(--space-6);
		background: rgba(16, 16, 16, 0.94);
		color: var(--text-primary);
		display: flex;
		flex-direction: column;
		gap: var(--space-6);
		border-right: 1px solid var(--border-default);
		backdrop-filter: blur(var(--blur-md));
	}

	.eyebrow {
		text-transform: uppercase;
		letter-spacing: 0.08em;
		font-size: 0.75rem;
		font-weight: 600;
		color: var(--text-secondary);
	}

	h1,
	h2,
	h3 {
		letter-spacing: -0.025em;
	}

	h1 {
		margin: 0;
		font-size: 2.25rem;
		line-height: 1.2;
		color: var(--text-hero);
	}

	.copy {
		line-height: 1.625;
		color: var(--text-secondary);
		max-width: 22rem;
	}

	.stack {
		display: grid;
		gap: var(--space-3);
	}

	button,
	select,
	input {
		font: inherit;
		font-family: var(--font-mono);
	}

	button {
		border: none;
		border-radius: var(--radius-sm);
		padding: 0.875rem 1rem;
		cursor: pointer;
		transition:
			background var(--duration-fast) var(--ease-standard),
			border-color var(--duration-fast) var(--ease-standard),
			color var(--duration-fast) var(--ease-standard),
			transform var(--duration-fast) var(--ease-standard);
	}

	button:disabled {
		opacity: 0.6;
		cursor: not-allowed;
	}

	.primary {
		background: var(--accent-200);
		color: var(--text-inverse);
	}

	.secondary {
		background: var(--bg-panel);
		color: var(--text-primary);
		border: 1px solid var(--border-default);
	}

	button:hover:not(:disabled) {
		transform: translateY(-1px);
	}

	.primary:hover:not(:disabled) {
		background: var(--accent-100);
	}

	.secondary:hover:not(:disabled) {
		border-color: var(--neutral-600);
	}

	.panel {
		padding: var(--space-4);
		border-radius: var(--radius-xs);
		background: var(--bg-panel);
		border: 1px solid var(--border-default);
		display: grid;
		gap: var(--space-3);
	}

	.panel-header {
		display: grid;
		gap: 0.25rem;
	}

	.panel-header span,
	.status {
		font-size: 0.75rem;
		text-transform: uppercase;
		letter-spacing: 0.08em;
		color: var(--text-secondary);
	}

	.panel-header strong {
		font-family: var(--font-mono);
		font-size: 0.75rem;
		font-weight: 500;
		color: var(--neutral-300);
		word-break: break-word;
	}

	.content {
		padding: var(--space-8);
		display: grid;
		gap: var(--space-6);
	}

	.topbar {
		display: flex;
		justify-content: space-between;
		gap: var(--space-4);
		align-items: end;
	}

	h2 {
		margin: 0;
		font-size: 1.875rem;
		color: var(--text-hero);
	}

	.search {
		min-width: 20rem;
	}

	.search input,
	select {
		width: 100%;
		border: 1px solid var(--border-default);
		border-radius: var(--radius-xs);
		background: var(--bg-panel);
		padding: 0.875rem 1rem;
		color: var(--text-primary);
		outline: none;
	}

	input::placeholder {
		color: var(--neutral-500);
	}

	.notes {
		background: linear-gradient(180deg, rgba(18, 18, 18, 0.96), rgba(10, 10, 10, 0.98));
		backdrop-filter: blur(var(--blur-sm));
		border-radius: var(--radius-sm);
		border: 1px solid var(--border-default);
		padding: var(--space-4);
	}

	.notes-header {
		display: flex;
		justify-content: space-between;
		align-items: center;
		padding-bottom: var(--space-3);
		border-bottom: 1px solid var(--border-subtle);
		margin-bottom: var(--space-3);
	}

	.notes-header span,
	.meta {
		font-family: var(--font-mono);
		font-size: 0.75rem;
		color: var(--text-secondary);
	}

	.library-controls,
	.library-actions {
		display: grid;
		gap: var(--space-2);
		margin-bottom: var(--space-3);
	}

	.library-controls {
		grid-template-columns: repeat(3, minmax(0, 1fr));
	}

	.library-actions {
		grid-template-columns: auto auto;
		justify-content: space-between;
		align-items: center;
	}

	.view-toggle {
		display: flex;
		gap: var(--space-2);
		flex-wrap: wrap;
	}

	.active-toggle {
		border-color: var(--accent-200);
		background: rgba(238, 96, 24, 0.08);
	}

	.notes-list {
		display: grid;
		gap: var(--space-2);
	}

	.notes-grid {
		grid-template-columns: repeat(auto-fill, minmax(12rem, 1fr));
		align-content: start;
	}

	.note-card {
		text-align: left;
		background: transparent;
		padding: var(--space-3);
		border: 1px solid transparent;
		color: var(--text-primary);
	}

	.note-card:hover {
		border-color: var(--accent-200);
		background: rgba(238, 96, 24, 0.08);
	}

	.note-card p {
		margin: var(--space-2) 0;
		color: var(--text-secondary);
		line-height: 1.4;
	}

	.meta {
		display: flex;
		justify-content: space-between;
		flex-wrap: wrap;
		gap: var(--space-2);
	}

	@keyframes fade-in {
		from {
			opacity: 0;
			transform: translateY(8px);
		}

		to {
			opacity: 1;
			transform: translateY(0);
		}
	}

	@media (max-width: 1100px) {
		.sidebar-toggle-btn {
			display: flex;
		}

		.sidebar-backdrop {
			display: block;
			position: fixed;
			inset: 0;
			background: rgba(0, 0, 0, 0.4);
			backdrop-filter: blur(var(--blur-sm));
			z-index: 90;
			animation: fade-in var(--duration-fast) ease-out;
		}

		.rail {
			position: absolute;
			top: 0; left: 0; bottom: 0;
			width: 320px;
			max-width: 85vw;
			z-index: 100;
			transform: translateX(-100%);
			transition: transform 0.3s cubic-bezier(0.16, 1, 0.3, 1);
			box-shadow: 4px 0 24px rgba(0, 0, 0, 0.4);
		}
		
		.rail.open {
			transform: translateX(0);
		}

		.library-controls,
		.library-actions {
			grid-template-columns: 1fr;
		}

		.topbar {
			flex-direction: column;
			align-items: stretch;
		}

		.content {
			padding: var(--space-4);
			width: 100%;
		}

		.search {
			min-width: 0;
		}
	}

	@media (min-width: 1101px) {
		.shell {
			display: grid;
			grid-template-columns: 320px 1fr;
		}
	}
</style>
