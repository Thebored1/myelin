<script lang="ts">
	import HomeDetailsAndTasks from '$lib/home/components/HomeDetailsAndTasks.svelte';
	import type { HomeController } from '$lib/home/controller.svelte';
	let { home }: { home: HomeController } = $props();
</script>

<main class="workspace">
	{#if !home.ready}
		<!-- Initial load: render nothing to avoid flashing the welcome screen. -->
	{:else if !home.app?.workspacePath}
		<div class="landing">
			<p class="eyebrow">Cross-platform local notes</p>
			<h1>myelin</h1>
			<p class="landing-copy">A local-first markdown workspace. Connect a folder to get started.</p>
			<button class="btn-primary" onclick={home.pickWorkspace} disabled={home.isBusy}
				>Choose workspace</button
			>
		</div>
	{:else}
		<div class="dashboard-container">
			<!-- Header -->
			<header class="dashboard-header">
				<div class="nb-switcher">
					<button
						class="nb-switch-btn"
						onclick={(e) => {
							e.stopPropagation();
							home.showNotebookMenu = !home.showNotebookMenu;
						}}
					>
						<h2>{home.activeNotebook ?? 'Uncategorized'}</h2>
						<svg
							class="nb-switch-caret"
							width="20"
							height="20"
							viewBox="0 0 24 24"
							fill="none"
							stroke="currentColor"
							stroke-width="2.5"
							stroke-linecap="round"
							stroke-linejoin="round"><polyline points="6 9 12 15 18 9" /></svg
						>
					</button>
					{#if home.showNotebookMenu}
						<div class="nb-switch-menu" role="presentation" onclick={(e) => e.stopPropagation()}>
							<button
								class="nb-switch-item"
								class:active={home.activeNotebook === null}
								onclick={() => {
									home.activeNotebook = null;
									home.showNotebookMenu = false;
								}}
							>
								<span>Uncategorized</span>
								<span class="nb-switch-count">{home.uncategorizedCount}</span>
							</button>
							{#each home.notebooks as nb (nb)}
								<button
									class="nb-switch-item"
									class:active={home.activeNotebook === nb}
									onclick={() => {
										home.activeNotebook = nb;
										home.showNotebookMenu = false;
									}}
								>
									<span class="ov-ellipsis">{nb}</span>
									<span class="nb-switch-count">{home.notebookCount(nb)}</span>
								</button>
							{/each}
							<div class="nb-add-divider"></div>
							<button
								class="nb-switch-item nb-switch-new"
								onclick={() => {
									home.showNotebookMenu = false;
									home.newNotebook();
								}}
							>
								<svg
									width="13"
									height="13"
									viewBox="0 0 24 24"
									fill="none"
									stroke="currentColor"
									stroke-width="2"
									stroke-linecap="round"
									stroke-linejoin="round"
									><line x1="12" y1="5" x2="12" y2="19" /><line
										x1="5"
										y1="12"
										x2="19"
										y2="12"
									/></svg
								>
								New notebook
							</button>
						</div>
					{/if}
				</div>
				<div style="display: flex; gap: var(--space-2); align-items: center;">
					<button
						class="header-toggle-btn"
						class:active={!home.tasksCollapsed && !home.selectedNote}
						onclick={() => {
							home.selectedNote = null;
							home.tasksCollapsed = !home.tasksCollapsed;
						}}
						title={home.tasksCollapsed ? 'Show tasks' : 'Hide tasks'}
					>
						<svg
							width="15"
							height="15"
							viewBox="0 0 24 24"
							fill="none"
							stroke="currentColor"
							stroke-width="2"
							stroke-linecap="round"
							stroke-linejoin="round"
							><polyline points="9 11 12 14 22 4" /><path
								d="M21 12v7a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h11"
							/></svg
						>
						Tasks
						{#if home.dashTasks.filter((t) => !t.done).length > 0}
							<span class="header-toggle-count">{home.dashTasks.filter((t) => !t.done).length}</span
							>
						{/if}
					</button>
				</div>
			</header>

			<div class="dashboard-grid">
				<!-- Left: notebook -->
				<div class="dash-left">
					<section class="dash-section nb-section">
						<h3 class="nb-header-tabs-container">
							<button
								class="h3-tab"
								class:active={home.activeTypeFilter !== 'documents'}
								onclick={() => home.setTypeFilter('notes')}
							>
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
								notes
							</button>
							<button
								class="h3-tab"
								class:active={home.activeTypeFilter === 'documents'}
								onclick={() => home.setTypeFilter('documents')}
							>
								<svg
									width="12"
									height="12"
									viewBox="0 0 24 24"
									fill="none"
									stroke="currentColor"
									stroke-width="2"
									stroke-linecap="round"
									stroke-linejoin="round"
									><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z" /><polyline
										points="14 2 14 8 20 8"
									/><line x1="16" y1="13" x2="8" y2="13" /><line
										x1="16"
										y1="17"
										x2="8"
										y2="17"
									/><polyline points="10 9 9 9 8 9" /></svg
								>
								documents
							</button>
							{#if home.activeNotebook}
								<button
									class="filter-chip"
									onclick={() => (home.activeNotebook = null)}
									title="Clear notebook filter"
								>
									{home.activeNotebook}<span class="chip-x">×</span>
								</button>
							{/if}
							{#if home.activeTag}
								<button
									class="filter-chip"
									onclick={() => (home.activeTag = null)}
									title="Clear tag filter"
								>
									#{home.activeTag}<span class="chip-x">×</span>
								</button>
							{/if}
						</h3>
						<div class="nb-list">
							{#each home.filteredNotebook as note (note.id)}
								<div
									class="nb-row"
									role="button"
									tabindex="0"
									class:selected={home.selectedNote?.id === note.id}
									onclick={() => home.selectOrOpen(note)}
									onkeydown={(event) => {
										if (event.key === 'Enter' || event.key === ' ') {
											event.preventDefault();
											home.selectOrOpen(note);
										}
									}}
									oncontextmenu={(e) => {
										e.preventDefault();
										home.activeMenuId = home.activeMenuId === note.id ? null : note.id;
									}}
								>
									{#if home.pinnedNoteIds.includes(note.id)}
										<svg
											class="nb-pin"
											width="11"
											height="11"
											viewBox="0 0 24 24"
											fill="currentColor"
											stroke="none"
											><path d="M19 21l-7-5-7 5V5a2 2 0 0 1 2-2h10a2 2 0 0 1 2 2z" /></svg
										>
									{/if}
									<span class="nb-row-title">{note.title}</span>
									{#if home.notebookOf(note)}
										<span class="nb-row-book" title="Notebook: {home.notebookOf(note)}">
											<svg
												width="10"
												height="10"
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
											{home.notebookOf(note)}
										</span>
									{/if}
									<span class="nb-row-date">{home.fullDateTime(note.createdAt)}</span>
									<span class="row-badge nb-row-badge">{home.getNoteBadge(note)}</span>
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
											<div
												class="row-dropdown"
												role="presentation"
												onclick={(e) => e.stopPropagation()}
											>
												<button
													class="row-delete"
													onclick={(e) => {
														e.preventDefault();
														home.togglePin(note.id);
													}}
													style="color: var(--text-primary);"
												>
													{home.pinnedNoteIds.includes(note.id) ? 'Unpin' : 'Pin'}
												</button>
												<button
													class="row-delete"
													onclick={(e) => home.requestDeleteNote(e, note.id)}>Delete</button
												>
											</div>
										{/if}
									</div>
								</div>
							{/each}
							{#if home.filteredNotebook.length === 0}
								{#if home.indexing && (home.app?.notes?.length ?? 0) > 0}
									<div class="nb-empty nb-home.indexing">
										<svg
											class="nb-spin"
											width="13"
											height="13"
											viewBox="0 0 24 24"
											fill="none"
											stroke="currentColor"
											stroke-width="2.5"
											stroke-linecap="round"><path d="M21 12a9 9 0 1 1-6.219-8.56" /></svg
										>
										Indexing your workspace…
									</div>
								{:else}
									<div class="nb-empty">
										{#if (home.app?.notes?.length ?? 0) === 0}
											No notes present.
										{:else}
											No {home.activeTypeFilter === 'all' ? '' : home.activeTypeFilter + ' '}notes
											yet.
										{/if}
									</div>
								{/if}
							{/if}
						</div>
					</section>
				</div>

				<HomeDetailsAndTasks {home} />
			</div>
		</div>
	{/if}
</main>
