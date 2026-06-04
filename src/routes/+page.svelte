<script lang="ts">
	import { invoke } from '@tauri-apps/api/core';
	import { listen } from '@tauri-apps/api/event';
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
	let selectedNote = $state<NoteDocument | null>(null);
	let draftBody = $state('');
	let draftTitle = $state('');
	let draftTags = $state('');
	let query = $state('');
	let sortMode = $state<'updated' | 'created' | 'title' | 'custom'>('updated');
	let viewMode = $state<'list' | 'grid'>('list');
	let filterFolder = $state('all');
	let filterTag = $state('all');
	let moveTarget = $state('');
	let isBusy = $state(false);
	let message = $state('Booting local-first workspace...');
	let searchResults = $state<SearchResponse | null>(null);
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
		app = await invoke<AppSnapshot>('bootstrap');
		provider = await invoke<ProviderStatus>('get_provider_status');
		if (!selectedNote && app.notes.length > 0) {
			await openNote(app.notes[0].id);
		}
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
				if (app.notes.length > 0) {
					await openNote(app.notes[0].id);
				} else {
					selectedNote = null;
					draftBody = '';
					draftTitle = '';
					draftTags = '';
				}
			} finally {
				isBusy = false;
			}
		}
	}

	async function openNote(noteId: string) {
		selectedNote = await invoke<NoteDocument>('load_note', { noteId });
		draftTitle = selectedNote.title;
		draftBody = selectedNote.body;
		draftTags = selectedNote.tags.join(', ');
	}

	async function createNote() {
		isBusy = true;
		try {
			selectedNote = await invoke<NoteDocument>('create_note', { title: 'New note' });
			draftTitle = selectedNote.title;
			draftBody = selectedNote.body;
			draftTags = '';
			await refreshApp();
			message = 'New note created.';
		} finally {
			isBusy = false;
		}
	}

	async function duplicateNote() {
		if (!selectedNote) return;
		isBusy = true;
		try {
			selectedNote = await invoke<NoteDocument>('duplicate_note', { noteId: selectedNote.id });
			await refreshApp();
			message = 'Note duplicated.';
		} finally {
			isBusy = false;
		}
	}

	async function saveNote() {
		if (!selectedNote) return;
		isBusy = true;
		try {
			selectedNote = await invoke<NoteDocument>('save_note', {
				noteId: selectedNote.id,
				title: draftTitle,
				tags: draftTags
					.split(',')
					.map((tag) => tag.trim())
					.filter(Boolean),
				body: draftBody
			});
			await refreshApp();
			message = 'Saved to markdown and reindexed.';
		} finally {
			isBusy = false;
		}
	}

	async function deleteNote() {
		if (!selectedNote) return;
		isBusy = true;
		try {
			app = await invoke<AppSnapshot>('delete_note', { noteId: selectedNote.id });
			selectedNote = null;
			draftTitle = '';
			draftBody = '';
			draftTags = '';
			if (app.notes.length > 0) {
				await openNote(app.notes[0].id);
			}
			message = 'Note deleted.';
		} finally {
			isBusy = false;
		}
	}

	async function moveNote() {
		if (!selectedNote || !moveTarget.trim()) return;
		isBusy = true;
		try {
			selectedNote = await invoke<NoteDocument>('move_note', {
				noteId: selectedNote.id,
				targetFolder: moveTarget
			});
			moveTarget = '';
			await refreshApp();
			message = 'Note moved.';
		} finally {
			isBusy = false;
		}
	}

	async function reorderSelected(direction: 'up' | 'down') {
		if (!selectedNote) return;
		isBusy = true;
		try {
			app = await invoke<AppSnapshot>('reorder_note', {
				noteId: selectedNote.id,
				direction
			});
			message = `Custom order updated (${direction}).`;
		} finally {
			isBusy = false;
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

	onMount(() => {
		let unlistenChanged = () => {};
		let unlistenStatus = () => {};

		void (async () => {
			await refreshApp();
			unlistenChanged = await listen('index://changed', () => {
				message = 'Filesystem change detected. Reindexing...';
			});
			unlistenStatus = await listen<string>('index://status', (event) => {
				message = event.payload === 'started' ? 'Indexing workspace...' : 'Workspace ready.';
				void refreshApp();
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
	<aside class="rail">
		<div>
			<p class="eyebrow">Cross-platform local notes</p>
			<h1>myelin</h1>
			<p class="copy">
				Rust owns storage, indexing, search, and future sync. The UI stays thin and desktop-native.
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
			<div>
				<p class="status">{message}</p>
				<h2>
					{app?.workspacePath
						? `${app.notes.length} notes indexed`
						: 'Connect a markdown workspace'}
				</h2>
			</div>

			<label class="search">
				<input
					bind:value={query}
					oninput={runSearch}
					placeholder="Search titles, tags, or meaning..."
				/>
			</label>
		</header>

		<div class="workspace">
			<section class="notes">
				<div class="notes-header">
					<h3>Notes</h3>
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
					<div class="quick-actions">
						<button class="secondary" onclick={createNote} disabled={isBusy || !app?.workspacePath}>
							Create
						</button>
						<button class="secondary" onclick={duplicateNote} disabled={isBusy || !selectedNote}>
							Duplicate
						</button>
						<button class="secondary" onclick={deleteNote} disabled={isBusy || !selectedNote}>
							Delete
						</button>
					</div>
				</div>

				<div class="move-row">
					<input bind:value={moveTarget} placeholder="folder/subfolder or Root" />
					<button class="secondary" onclick={moveNote} disabled={isBusy || !selectedNote}>
						Move
					</button>
				</div>

				{#if sortMode === 'custom'}
					<div class="custom-order-row">
						<button
							class="secondary"
							onclick={() => reorderSelected('up')}
							disabled={isBusy || !selectedNote}
						>
							Move up
						</button>
						<button
							class="secondary"
							onclick={() => reorderSelected('down')}
							disabled={isBusy || !selectedNote}
						>
							Move down
						</button>
					</div>
				{/if}

				<div class:notes-grid={viewMode === 'grid'} class="notes-list">
					{#each visibleNotes as note (note.id)}
						<button
							class:selected={selectedNote?.id === note.id}
							class="note-card"
							onclick={() => openNote(note.id)}
						>
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

			<section class="editor">
				{#if selectedNote}
					<div class="editor-toolbar">
						<input class="title-input" bind:value={draftTitle} placeholder="Note title" />
						<div class="toolbar-actions">
							<button class="secondary" onclick={deleteNote} disabled={isBusy}>Delete</button>
							<button class="primary" onclick={saveNote} disabled={isBusy}>Save</button>
						</div>
					</div>
					<input class="tag-input" bind:value={draftTags} placeholder="comma,separated,tags" />
					<textarea bind:value={draftBody} placeholder="Write markdown here..."></textarea>
				{:else}
					<div class="empty">
						<h3>Rust core is ready</h3>
						<p>Select a workspace and create a note to start building the knowledge base.</p>
					</div>
				{/if}
			</section>
		</div>
	</section>
</div>

<style>
	.shell {
		display: grid;
		grid-template-columns: 320px 1fr;
		min-height: 100vh;
		animation: fade-in var(--duration-page) var(--ease-out);
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

	button {
		border: none;
		border-radius: var(--radius-sm);
		padding: 0.875rem 1rem;
		font: inherit;
		font-family: var(--font-mono);
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

	.panel-header span {
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

	.status {
		margin: 0 0 4px;
		font-size: 0.75rem;
		color: var(--text-secondary);
		text-transform: uppercase;
		letter-spacing: 0.05em;
	}

	h2 {
		margin: 0;
		font-size: 1.875rem;
		color: var(--text-hero);
	}

	.search input,
	.title-input,
	.tag-input,
	textarea {
		width: 100%;
		border: 1px solid var(--border-default);
		border-radius: var(--radius-xs);
		background: var(--bg-panel);
		padding: 0.875rem 1rem;
		font: inherit;
		font-family: var(--font-mono);
		color: var(--text-primary);
		outline: none;
		transition:
			border-color var(--duration-fast) var(--ease-standard),
			box-shadow var(--duration-fast) var(--ease-standard);
	}

	input::placeholder,
	textarea::placeholder {
		color: var(--neutral-500);
	}

	.search input:focus,
	.title-input:focus,
	.tag-input:focus,
	textarea:focus {
		border-color: var(--accent-200);
		box-shadow: 0 0 0 1px rgba(238, 96, 24, 0.28);
	}

	.workspace {
		display: grid;
		grid-template-columns: 360px 1fr;
		gap: var(--space-5);
		min-height: 0;
	}

	.notes,
	.editor {
		background: linear-gradient(180deg, rgba(18, 18, 18, 0.96), rgba(10, 10, 10, 0.98));
		backdrop-filter: blur(var(--blur-sm));
		border-radius: var(--radius-sm);
		border: 1px solid var(--border-default);
		padding: var(--space-4);
	}

	.notes {
		display: grid;
		grid-template-rows: auto 1fr;
	}

	.notes-header {
		display: flex;
		justify-content: space-between;
		align-items: center;
		padding-bottom: var(--space-3);
		border-bottom: 1px solid var(--border-subtle);
		margin-bottom: var(--space-3);
	}

	.notes-list {
		display: grid;
		gap: var(--space-2);
		overflow: auto;
		max-height: calc(100vh - 220px);
	}

	.notes-grid {
		grid-template-columns: repeat(auto-fill, minmax(12rem, 1fr));
		align-content: start;
	}

	.library-controls,
	.library-actions,
	.move-row,
	.custom-order-row {
		display: grid;
		gap: var(--space-2);
		margin-bottom: var(--space-3);
	}

	.library-controls {
		grid-template-columns: repeat(3, minmax(0, 1fr));
	}

	.library-actions {
		grid-template-columns: auto 1fr;
		align-items: center;
	}

	.quick-actions,
	.view-toggle,
	.custom-order-row {
		display: flex;
		gap: var(--space-2);
		flex-wrap: wrap;
	}

	.move-row {
		grid-template-columns: 1fr auto;
	}

	select,
	.move-row input {
		width: 100%;
		border: 1px solid var(--border-default);
		border-radius: var(--radius-xs);
		background: var(--bg-panel);
		padding: 0.875rem 1rem;
		font: inherit;
		font-family: var(--font-mono);
		color: var(--text-primary);
		outline: none;
	}

	.active-toggle {
		border-color: var(--accent-200);
		background: rgba(238, 96, 24, 0.08);
	}

	.note-card {
		text-align: left;
		background: transparent;
		padding: var(--space-3);
		border: 1px solid transparent;
		color: var(--text-primary);
		font-family: var(--font-mono);
	}

	.note-card.selected {
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
		font-size: 0.75rem;
		font-family: var(--font-mono);
		color: var(--neutral-500);
		gap: var(--space-2);
	}

	.editor {
		display: grid;
		grid-template-rows: auto auto 1fr;
		gap: var(--space-3);
	}

	.editor-toolbar {
		display: flex;
		justify-content: space-between;
		gap: var(--space-3);
		align-items: center;
	}

	.title-input {
		font-size: 1.5rem;
		font-weight: 600;
		color: var(--text-hero);
	}

	.toolbar-actions {
		display: flex;
		gap: var(--space-2);
	}

	textarea {
		min-height: 56vh;
		resize: vertical;
		line-height: 1.65;
	}

	.empty {
		display: grid;
		place-items: center;
		align-content: center;
		min-height: 50vh;
		text-align: center;
		color: var(--text-secondary);
		border: 1px dashed var(--border-subtle);
		border-radius: var(--radius-xs);
		background: rgba(255, 255, 255, 0.02);
	}

	.search {
		min-width: 20rem;
	}

	.notes-header span {
		font-family: var(--font-mono);
		font-size: 0.75rem;
		color: var(--text-secondary);
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

	@media (max-width: 980px) {
		.shell,
		.workspace {
			grid-template-columns: 1fr;
		}

		.topbar,
		.editor-toolbar {
			flex-direction: column;
			align-items: stretch;
		}

		.content {
			padding: var(--space-4);
		}

		.rail {
			padding: var(--space-5) var(--space-4);
		}

		.search {
			min-width: 0;
		}

		.library-controls,
		.library-actions,
		.move-row {
			grid-template-columns: 1fr;
		}
	}
</style>
