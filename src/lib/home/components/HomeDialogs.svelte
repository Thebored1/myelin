<script lang="ts">
import type { HomeController } from '$lib/home/controller.svelte';
let { home }: { home: HomeController } = $props();
</script>


<dialog
	bind:this={home.deleteDialog}
	class="confirm-dialog"
	onclose={() => {
		home.noteToDelete = null;
	}}
>
	<div class="dialog-content">
		<h3>Delete note?</h3>
		<p>This cannot be undone.</p>
		<div class="dialog-actions">
			<button class="btn-ghost" onclick={() => home.deleteDialog?.close()}>Cancel</button>
			<button class="btn-danger" onclick={home.confirmDelete} disabled={home.isBusy}>Delete</button>
		</div>
		</div>
</dialog>

<dialog
	bind:this={home.notebookDialog}
	class="confirm-dialog"
	onclose={() => {
		home.newNotebookName = '';
	}}
>
	<div class="dialog-content">
		<h3>New notebook</h3>
		<p>Create a notebook — a folder that holds notes of any kind.</p>
		<input
			class="nb-name-input"
			bind:value={home.newNotebookName}
			use:home.autofocus
			placeholder="Notebook name"
			onkeydown={(e) => {
				if (e.key === 'Enter') {
					e.preventDefault();
					home.confirmNewNotebook();
				}
			}}
		/>
		<div class="dialog-actions">
			<button class="btn-ghost" onclick={() => home.notebookDialog?.close()}>Cancel</button>
			<button
				class="btn-primary nb-create-btn"
				onclick={home.confirmNewNotebook}
				disabled={home.isBusy || !home.newNotebookName.trim()}>Create</button
			>
		</div>
		</div>
	</dialog>

<dialog
	bind:this={home.globalSearchDialog}
	class="link-dialog"
	onkeydown={home.handleGlobalSearchKeydown}
	onclose={() => {
		home.globalSearchQuery = '';
		home.globalSelectedIndex = 0;
	}}
>
	<div class="dialog-content">
		<h3>Link to Note</h3>
		<p style="font-size: 0.875rem; color: var(--text-secondary); margin-bottom: var(--space-4);">
			Search and select a note from your library.
		</p>

		<input
			class="link-search-input"
			bind:value={home.globalSearchQuery}
			oninput={() => (home.globalSelectedIndex = 0)}
			use:home.autofocus
			placeholder="Search notes..."
		/>

		<div class="link-results-container">
			{#if home.filteredGlobalNotes.length > 0}
				<ul class="link-results-list">
					{#each home.filteredGlobalNotes as res, i (res.id + '_' + i)}
						<li>
							<button
								class="link-result-btn"
								class:selected={i === home.globalSelectedIndex}
								onclick={() => home.openNoteFromSearch(res)}
							>
								<strong>{res.title}</strong>
								<span class="folder-badge">{home.folderFromRelativePath(res.relativePath)}</span>
							</button>
						</li>
					{/each}
				</ul>
			{:else if home.globalSearchQuery.trim()}
				<p class="empty-state">No notes found matching your search.</p>
			{/if}
		</div>

		<div class="dialog-actions">
			<button class="btn-ghost" onclick={() => home.globalSearchDialog?.close()}>Cancel</button>
		</div>
	</div>
</dialog>

{#if home.isClustersListOpen}
	<!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
	<div class="modal-overlay" onclick={home.closeClustersList}>
		<div class="modal-content" onclick={(e) => e.stopPropagation()}>
			<header class="modal-header">
				<h2>Clusters ({home.commonplaces.length})</h2>
				<p class="modal-subtitle">
					Groups of notes connected by links. Select one to view its notes.
				</p>
			</header>
			<div class="modal-body">
				{#if home.commonplaces.length === 0}
					<p class="cluster-empty">No clusters yet. Link notes together to form connections.</p>
				{:else}
					<div class="cluster-list">
						{#each home.commonplaces as cluster, i}
							<button
								class="cluster-row"
								onclick={() => {
									home.closeClustersList();
									home.openCluster(cluster);
								}}
							>
								<span class="cluster-name">
									<svg
										width="13"
										height="13"
										viewBox="0 0 24 24"
										fill="none"
										stroke="currentColor"
										stroke-width="2"
										stroke-linecap="round"
										stroke-linejoin="round"
										><circle cx="12" cy="12" r="3" /><circle cx="5" cy="6" r="2" /><circle
											cx="19"
											cy="6"
											r="2"
										/><circle cx="6" cy="19" r="2" /><path
											d="M9.5 10.5 6.7 7.3M14.5 10.5l2.8-3.2M10.2 14.2 7.4 17.4"
										/></svg
									>
									Cluster {i + 1}
								</span>
								<span class="cluster-meta">{cluster.length} notes</span>
							</button>
						{/each}
					</div>
				{/if}
			</div>
			<footer class="modal-footer">
				<button class="btn-cancel" onclick={home.closeClustersList}>Close</button>
			</footer>
		</div>
	</div>
{/if}

{#if home.isClusterDialogOpen}
	<!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
	<div class="modal-overlay" onclick={home.closeClusterDialog}>
		<div class="modal-content" onclick={(e) => e.stopPropagation()}>
			<header class="modal-header">
				<h2>Cluster Notes ({home.selectedCluster.length})</h2>
				<p class="modal-subtitle">Select a note to view its contents.</p>
			</header>
			<div class="modal-body">
				<div class="table-list">
					<div class="table-header">
						<div class="th-col">Title</div>
						<div class="th-col">Folder</div>
						<div class="th-col">Created</div>
						<div class="th-col">Modified</div>
					</div>
					{#each home.selectedCluster as note}
						<!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
						<div
							class="table-row"
							onclick={() => {
								home.closeClusterDialog();
								home.openNote(note.id);
							}}
						>
							<div class="td-col td-primary">{note.title}</div>
							<div class="td-col">{note.folder || 'root'}</div>
							<div class="td-col">{new Date(note.createdAt).toLocaleDateString()}</div>
							<div class="td-col">{new Date(note.updatedAt).toLocaleDateString()}</div>
						</div>
					{/each}
				</div>
			</div>
			<footer class="modal-footer">
				<button class="btn-cancel" onclick={home.closeClusterDialog}>Cancel</button>
			</footer>
		</div>
	</div>
{/if}
