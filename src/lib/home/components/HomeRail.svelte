<script lang="ts">
	import { theme, toggleTheme } from '$lib/theme';
	import type { HomeController } from '$lib/home/controller.svelte';
	let { home }: { home: HomeController } = $props();
</script>

<aside class="rail">
	<div class="rail-top">
		<span class="wordmark">myelin</span>
		<button
			class="theme-toggle-btn"
			onclick={toggleTheme}
			title={$theme === 'light' ? 'Switch to dark mode' : 'Switch to light mode'}
			aria-label="Toggle theme"
		>
			{#if $theme === 'light'}
				<!-- moon: click to go dark -->
				<svg
					width="16"
					height="16"
					viewBox="0 0 24 24"
					fill="none"
					stroke="currentColor"
					stroke-width="2"
					stroke-linecap="round"
					stroke-linejoin="round"
				>
					<path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z"></path>
				</svg>
			{:else}
				<!-- sun: click to go light -->
				<svg
					width="16"
					height="16"
					viewBox="0 0 24 24"
					fill="none"
					stroke="currentColor"
					stroke-width="2"
					stroke-linecap="round"
					stroke-linejoin="round"
				>
					<circle cx="12" cy="12" r="4"></circle>
					<path
						d="M12 2v2M12 20v2M4.93 4.93l1.41 1.41M17.66 17.66l1.41 1.41M2 12h2M20 12h2M6.34 17.66l-1.41 1.41M19.07 4.93l-1.41 1.41"
					></path>
				</svg>
			{/if}
		</button>
	</div>

	{#if home.app?.workspacePath}
		<button class="rail-search-btn" onclick={() => home.globalSearchDialog?.showModal()}>
			<svg
				width="14"
				height="14"
				viewBox="0 0 24 24"
				fill="none"
				stroke="currentColor"
				stroke-width="2"
				stroke-linecap="round"
				><circle cx="11" cy="11" r="8" /><line x1="21" y1="21" x2="16.65" y2="16.65" /></svg
			>
			Search notes...
		</button>
	{/if}

	<div class="rail-list">
		{#if !home.ready}
			<!-- Loading: no empty-state flash. -->
		{:else if !home.app?.workspacePath}
			<p class="rail-empty">No workspace selected.</p>
		{:else if home.query.trim()}
			{#if home.visibleNotes.length === 0}
				<p class="rail-empty">No results for “{home.query}”.</p>
			{/if}
			{#if home.visibleNotes.length > 0}
				<div class="section-label">
					<span>All Notes</span>
					<span class="section-count">{home.visibleNotes.length}</span>
				</div>
				{#each home.visibleNotes as note (note.id)}
					<div
						class="note-row"
						role="button"
						tabindex="0"
						onclick={() => home.openNote(note.id)}
						onkeydown={(event) => {
							if (event.key === 'Enter' || event.key === ' ') {
								event.preventDefault();
								home.openNote(note.id);
							}
						}}
						oncontextmenu={(e) => {
							e.preventDefault();
							home.activeMenuId = home.activeMenuId === note.id ? null : note.id;
						}}
					>
						<svg
							class="row-icon {note.relativePath.toLowerCase().endsWith('.pdf') ? 'pdf-icon' : ''}"
							width="14"
							height="14"
							viewBox="0 0 24 24"
							fill="none"
							stroke="currentColor"
							stroke-width="2"
							stroke-linecap="round"
							><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z" /><polyline
								points="14 2 14 8 20 8"
							/>{#if note.relativePath.toLowerCase().endsWith('.pdf')}<line
									x1="9"
									y1="13"
									x2="15"
									y2="13"
								/><line x1="9" y1="17" x2="11" y2="17" />{/if}</svg
						>
						<span class="row-title">{note.title}</span>
						<div class="row-badge">
							{home.getNoteBadge(note)}
						</div>
						<span class="row-time">{home.timeAgo(note.createdAt)}</span>
						<div class="row-menu-wrap">
							<button
								class="row-menu-btn"
								onclick={(e) => {
									e.stopPropagation();
									home.activeMenuId = home.activeMenuId === note.id ? null : note.id;
								}}
								aria-label="Options"
							>
								<svg width="12" height="12" viewBox="0 0 24 24" fill="currentColor"
									><circle cx="12" cy="5" r="1.5" /><circle cx="12" cy="12" r="1.5" /><circle
										cx="12"
										cy="19"
										r="1.5"
									/></svg
								>
							</button>
							{#if home.activeMenuId === note.id}
								<div class="row-dropdown">
									<button class="row-delete" onclick={(e) => home.requestDeleteNote(e, note.id)}
										>Delete</button
									>
								</div>
							{/if}
						</div>
					</div>
				{/each}
			{/if}
		{:else}
			<div class="section-label">
				<span>New</span>
			</div>
			<div class="ov-group new-actions">
				<button
					class="ov-row ov-clickable ov-action"
					onclick={() => home.createNote('md')}
					disabled={home.isBusy}
				>
					<span class="ov-key"
						><svg
							width="11"
							height="11"
							viewBox="0 0 24 24"
							fill="none"
							stroke="currentColor"
							stroke-width="2"
							stroke-linecap="round"
							stroke-linejoin="round"
							><path d="M12 20h9" /><path d="M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4z" /></svg
						>markdown</span
					>
				</button>
				<button
					class="ov-row ov-clickable ov-action"
					onclick={() => home.createNote('tex')}
					disabled={home.isBusy}
				>
					<span class="ov-key"
						><svg
							width="11"
							height="11"
							viewBox="0 0 24 24"
							fill="none"
							stroke="currentColor"
							stroke-width="2"
							stroke-linecap="round"
							stroke-linejoin="round"><path d="M18 4H6l6 8-6 8h12" /></svg
						>latex</span
					>
				</button>
				<button
					class="ov-row ov-clickable ov-action"
					onclick={() => home.createNote('ipynb')}
					disabled={home.isBusy}
				>
					<span class="ov-key"
						><svg
							width="11"
							height="11"
							viewBox="0 0 24 24"
							fill="none"
							stroke="currentColor"
							stroke-width="2"
							stroke-linecap="round"
							stroke-linejoin="round"
							><polyline points="16 18 22 12 16 6" /><polyline points="8 6 2 12 8 18" /></svg
						>jupyter</span
					>
				</button>
				<button
					class="ov-row ov-clickable ov-action"
					onclick={home.importFile}
					disabled={home.isBusy}
				>
					<span class="ov-key"
						><svg
							width="11"
							height="11"
							viewBox="0 0 24 24"
							fill="none"
							stroke="currentColor"
							stroke-width="2"
							stroke-linecap="round"
							stroke-linejoin="round"
							><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" /><polyline
								points="17 8 12 3 7 8"
							/><line x1="12" y1="3" x2="12" y2="15" /></svg
						>upload</span
					>
				</button>
				<button class="new-notebook-btn" onclick={home.newNotebook} disabled={home.isBusy}>
					<svg
						width="12"
						height="12"
						viewBox="0 0 24 24"
						fill="none"
						stroke="currentColor"
						stroke-width="2"
						stroke-linecap="round"
						stroke-linejoin="round"
						><path d="M2 3h6a4 4 0 0 1 4 4v14a3 3 0 0 0-3-3H2z" /><path
							d="M22 3h-6a4 4 0 0 0-4 4v14a3 3 0 0 1 3-3h7z"
						/></svg
					>
					new notebook
				</button>
			</div>

			<!-- Details panel — the page handles browsing, the rail shows workspace vitals -->
			<div class="section-label">
				<span>Library</span>
			</div>
			<div class="ov-group">
				<!-- home.notebooks group -->
				<button
					class="ov-row ov-group-head"
					onclick={() => (home.notebooksExpanded = !home.notebooksExpanded)}
				>
					<span class="ov-key">
						<svg
							class="ov-chevron"
							class:collapsed={!home.notebooksExpanded}
							width="11"
							height="11"
							viewBox="0 0 24 24"
							fill="none"
							stroke="currentColor"
							stroke-width="2.5"
							stroke-linecap="round"
							stroke-linejoin="round"><polyline points="6 9 12 15 18 9" /></svg
						>
						home.notebooks
					</span>
					<span class="ov-val">{home.notebooks.length}</span>
				</button>
				{#if home.notebooksExpanded}
					<button
						class="ov-row ov-sub ov-clickable"
						class:active={home.activeNotebook === null}
						onclick={() => (home.activeNotebook = null)}
						title="Notes not in a notebook"
					>
						<span class="ov-key ov-ellipsis">uncategorized</span>
						<span class="ov-val">{home.uncategorizedCount}</span>
					</button>
					{#each home.notebooks as nb (nb)}
						<button
							class="ov-row ov-sub ov-clickable"
							class:active={home.activeNotebook === nb}
							onclick={() => home.toggleNotebook(nb)}
							title={nb}
						>
							<span class="ov-key ov-ellipsis">{nb}</span>
							<span class="ov-val">{home.notebookCount(nb)}</span>
						</button>
					{/each}
				{/if}

				<!-- notes group: editable working docs -->
				<button
					class="ov-row ov-group-head"
					onclick={() => (home.notesExpanded = !home.notesExpanded)}
				>
					<span class="ov-key">
						<svg
							class="ov-chevron"
							class:collapsed={!home.notesExpanded}
							width="11"
							height="11"
							viewBox="0 0 24 24"
							fill="none"
							stroke="currentColor"
							stroke-width="2.5"
							stroke-linecap="round"
							stroke-linejoin="round"><polyline points="6 9 12 15 18 9" /></svg
						>
						notes
					</span>
					<span class="ov-val"
						>{home.typeCounts.md + home.typeCounts.tex + home.typeCounts.ipynb}</span
					>
				</button>
				{#if home.notesExpanded}
					{#each home.NOTE_SUBTYPES as st (st.type)}
						{#if home.typeCounts[st.type] > 0}
							<button
								class="ov-row ov-sub ov-clickable"
								class:active={home.activeTypeFilter === st.type}
								onclick={() =>
									home.setTypeFilter(home.activeTypeFilter === st.type ? 'all' : st.type)}
							>
								<span class="ov-key ov-ellipsis">{st.label}</span>
								<span class="ov-val">{home.typeCounts[st.type]}</span>
							</button>
						{/if}
					{/each}
				{/if}

				<!-- documents group: uploaded source material -->
				<button
					class="ov-row ov-group-head"
					onclick={() => (home.docsExpanded = !home.docsExpanded)}
				>
					<span class="ov-key">
						<svg
							class="ov-chevron"
							class:collapsed={!home.docsExpanded}
							width="11"
							height="11"
							viewBox="0 0 24 24"
							fill="none"
							stroke="currentColor"
							stroke-width="2.5"
							stroke-linecap="round"
							stroke-linejoin="round"><polyline points="6 9 12 15 18 9" /></svg
						>
						documents
					</span>
					<span class="ov-val">{home.typeCounts.pdf + home.typeCounts.epub}</span>
				</button>
				{#if home.docsExpanded}
					{#each home.DOC_SUBTYPES as st (st.type)}
						{#if home.typeCounts[st.type] > 0}
							<button
								class="ov-row ov-sub ov-clickable"
								class:active={home.activeTypeFilter === st.type}
								onclick={() =>
									home.setTypeFilter(home.activeTypeFilter === st.type ? 'all' : st.type)}
							>
								<span class="ov-key ov-ellipsis">{st.label}</span>
								<span class="ov-val">{home.typeCounts[st.type]}</span>
							</button>
						{/if}
					{/each}
				{/if}

				<div class="ov-row">
					<span class="ov-key">tags</span><span class="ov-val">{home.tagCounts.length}</span>
				</div>
				<button class="ov-row ov-clickable" onclick={home.openClustersDialog} title="View clusters"
					><span class="ov-key">clusters</span><span class="ov-val">{home.commonplaces.length}</span
					></button
				>
			</div>

			<div class="section-label" style="margin-top: var(--space-4);">
				<span>Tags</span>
				<span class="section-count">{home.tagCounts.length}</span>
			</div>
			{#if home.tagCounts.length > 0}
				<div class="ov-group">
					{#each home.tagCounts.slice(0, 12) as [tag, count] (tag)}
						<button
							class="ov-row ov-clickable"
							class:active={home.activeTag === tag}
							onclick={() => home.toggleTag(tag)}
							title="Filter by #{tag}"
						>
							<span class="ov-key ov-ellipsis">#{tag}</span>
							<span class="ov-val">{count}</span>
						</button>
					{/each}
				</div>
			{:else}
				<p class="rail-empty">No tags yet.</p>
			{/if}

			<div class="section-label" style="margin-top: var(--space-4);">
				<span>System</span>
			</div>
			<div class="ov-group">
				<div class="ov-row">
					<span class="ov-key">index</span><span class="ov-val"
						>{home.app?.indexState.backend ?? '—'}</span
					>
				</div>
				<div class="ov-row">
					<span class="ov-key">indexed</span>
					<span class="ov-val"
						>{home.app?.indexState.lastIndexedAt
							? home.agoLabel(home.app.indexState.lastIndexedAt)
							: '—'}</span
					>
				</div>
				<div class="ov-row">
					<span class="ov-key">home.provider</span>
					<span class="ov-val" class:ov-ok={home.provider?.healthy}
						>{home.provider?.activeProvider || 'none'}</span
					>
				</div>
				<div class="ov-row">
					<span class="ov-key">model</span>
					<span
						class="ov-val ov-ellipsis ov-model"
						title={home.provider?.resolved?.modelPath ??
							home.provider?.config?.modelPath ??
							'No model selected'}
						>{home.modelFileName(
							home.provider?.resolved?.modelPath ?? home.provider?.config?.modelPath
						)}</span
					>
				</div>
				<div class="ov-row">
					<span class="ov-key">embedding</span>
					<span
						class="ov-val ov-ellipsis ov-model"
						title={home.embeddingModelPath ?? 'No embedding model selected'}
						>{home.modelFileName(home.embeddingModelPath)}</span
					>
				</div>
			</div>
		{/if}
	</div>

	{#if home.appVersion}
		<div class="rail-version">v{home.appVersion}</div>
	{/if}

	<div class="rail-footer">
		<button
			class="footer-change-btn"
			onclick={home.pickWorkspace}
			disabled={home.isBusy}
			title={home.app?.workspacePath ?? 'Choose workspace'}
		>
			<svg
				width="11"
				height="11"
				viewBox="0 0 24 24"
				fill="none"
				stroke="currentColor"
				stroke-width="2"
				stroke-linecap="round"
				><path
					d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"
				/></svg
			>
			{#if !home.ready}
				&nbsp;
			{:else if home.app?.workspacePath}
				{home.workspaceLabel(home.app.workspacePath)}
			{:else}
				Connect workspace
			{/if}
		</button>
		{#if home.app?.workspacePath}
			<span
				class="footer-dot"
				class:dot-ok={home.provider?.activeProvider}
				title={home.provider?.activeProvider ?? 'No home.provider'}
			></span>
		{/if}
	</div>
</aside>
