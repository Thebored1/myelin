<script lang="ts">
	import type { NotePageController } from '$lib/note-page/controller.svelte';

	let { notePage }: { notePage: NotePageController } = $props();
</script>

<dialog
	bind:this={notePage.versionPreviewDialog}
	class="version-preview-dialog"
	onclose={() => (notePage.versionPreviewContent = null)}
>
	<div class="dialog-content" style="max-width: 800px; width: 90vw;">
		<h3>Version Preview</h3>
		<div
			class="preview-content"
			style="max-height: 60vh; overflow-y: auto; background: var(--bg-page); padding: 1rem; border-radius: var(--radius-sm); border: 1px solid var(--border-default); white-space: pre-wrap; font-family: var(--font-mono); font-size: 0.875rem; margin: 1rem 0;"
		>
			{notePage.versionPreviewContent || 'Loading...'}
		</div>
		<div class="dialog-actions">
			<button class="secondary" onclick={() => notePage.versionPreviewDialog?.close()}>Close</button>
			{#if notePage.versionPreviewHash}
				<button class="primary" onclick={() => notePage.restoreVersion(notePage.versionPreviewHash!)}
					>Restore This Version</button
				>
			{/if}
		</div>
	</div>
</dialog>

<dialog
	bind:this={notePage.mathDialog}
	class="math-dialog"
	onclose={() => {
		notePage.mathValue = '';
		notePage.mathError = '';
	}}
>
	<div class="dialog-content">
		<h3>Insert Math</h3>
		<div class="math-container">
			{#if notePage.mathLiveReady}
				<svelte:element
					this={'math-field'}
					oninput={(e: any) => (notePage.mathValue = e.target.value)}
					style="width: 100%; font-size: 1.5rem; padding: 0.5rem; background: var(--bg-panel); color: var(--text-primary); border: 1px solid var(--border-default); border-radius: var(--radius-xs);"
					>{notePage.mathValue}</svelte:element
				>
			{:else}
				<span class="editor-loading">Loading math editor…</span>
			{/if}
		</div>
		{#if notePage.mathError}
			<p style="margin: 8px 0 0; font-size: 0.8rem; color: var(--danger, #e5534b);">
				⚠ Won't render in the note: {notePage.mathError}
			</p>
		{/if}
		<div class="dialog-actions">
			<button class="secondary" onclick={() => notePage.mathDialog?.close()}>Cancel</button>
			<button class="primary" onclick={notePage.insertMath} disabled={!notePage.mathValue}
				>{notePage.mathError ? 'Insert anyway' : 'Insert'}</button
			>
		</div>
	</div>
</dialog>

<dialog
	bind:this={notePage.linkNoteDialog}
	class="link-dialog"
	onkeydown={notePage.handleLinkSearchKeydown}
	onclose={() => {
		notePage.linkSearchQuery = '';
		notePage.linkSearchResults = [];
		notePage.linkSelectedIndex = 0;
		notePage.linkDialogMode = 'notes';
		if (notePage.shouldRefocusEditor) notePage.refocusEditorSoon();
	}}
>
	<div class="dialog-content">
		{#if notePage.linkDialogMode === 'notes'}
			<h3>Link to Note</h3>
			<p style="font-size: 0.875rem; color: var(--text-secondary); margin-bottom: var(--space-4);">
				Search and select a note to link your highlighted text to.
			</p>

			<input
				class="link-search-input"
				bind:value={notePage.linkSearchQuery}
				oninput={() => (notePage.linkSelectedIndex = 0)}
				use:notePage.autofocus
				placeholder="Search notes..."
			/>

			{#if notePage.linkSearchQuery.trim() || notePage.linkSearchResults.length > 0}
				<div class="link-results-container">
					{#if notePage.linkSearchResults.length > 0}
						<ul class="link-results-list">
							{#each notePage.linkSearchResults as res, i (res.id + '_' + i)}
								<li>
									<button
										class="link-result-btn"
										class:selected={i === notePage.linkSelectedIndex}
										onclick={() => notePage.selectNoteForBlocks(res)}
									>
										<strong>{res.title}</strong>
										<span class="folder-badge">{res.folder}</span>
									</button>
								</li>
							{/each}
						</ul>
					{:else if notePage.linkSearchQuery.trim()}
						<p class="empty-state">No notes found matching your search.</p>
					{/if}
				</div>
			{/if}
		{:else}
			<h3>Select Block to Reference</h3>
			<p style="font-size: 0.875rem; color: var(--text-secondary); margin-bottom: var(--space-4);">
				Select a specific block from <strong>{notePage.selectedNoteForBlocks?.title}</strong> or link the entire
				note.
			</p>

			<input
				class="link-search-input"
				bind:value={notePage.linkSearchQuery}
				oninput={() => (notePage.linkSelectedIndex = 0)}
				use:notePage.autofocus
				placeholder="Search blocks..."
			/>

			<div class="link-results-container">
				{#if notePage.filteredBlocks.length > 0}
					<ul class="link-results-list">
						{#each notePage.filteredBlocks as block, i}
							<li>
								<button
									class="link-result-btn"
									class:selected={i === notePage.linkSelectedIndex}
									onclick={() => notePage.insertBlockLink(block)}
								>
									<span
										style={block.isFullNote
											? 'font-weight: bold;'
											: 'font-size: 0.9em; opacity: 0.9; display: -webkit-box; -webkit-line-clamp: 2; -webkit-box-orient: vertical; overflow: hidden;'}
									>
										{block.text}
									</span>
								</button>
							</li>
						{/each}
					</ul>
				{:else}
					<p class="empty-state">No matching blocks found.</p>
				{/if}
			</div>
		{/if}

		<div class="dialog-actions">
			{#if notePage.linkDialogMode === 'blocks'}
				<button
					class="secondary"
					style="margin-right: auto;"
					onclick={() => {
						notePage.linkDialogMode = 'notes';
						notePage.linkSearchQuery = '';
						notePage.linkSelectedIndex = 0;
					}}>Back</button
				>
			{/if}
			<button class="secondary" onclick={() => notePage.linkNoteDialog?.close()}>Cancel</button>
		</div>
	</div>
</dialog>

<dialog
	bind:this={notePage.globalSearchDialog}
	class="link-dialog"
	onkeydown={notePage.handleGlobalSearchKeydown}
	onclose={() => {
		notePage.globalSearchQuery = '';
		notePage.globalSelectedIndex = 0;
		notePage.globalBlocks = [];
		if (notePage.shouldRefocusEditor) notePage.refocusEditorSoon();
	}}
>
	<div class="dialog-content">
		<h3>Search Global Blocks</h3>
		<p style="font-size: 0.875rem; color: var(--text-secondary); margin-bottom: var(--space-4);">
			Search blocks across all notes.
		</p>

		<input
			class="link-search-input"
			bind:value={notePage.globalSearchQuery}
			oninput={() => (notePage.globalSelectedIndex = 0)}
			placeholder="Search global blocks..."
		/>

		<div class="link-results-container">
			{#if notePage.filteredGlobalBlocks.length > 0}
				<ul class="link-results-list">
					{#each notePage.filteredGlobalBlocks as block, i}
						<li>
							<button
								class="link-result-btn"
								class:selected={i === notePage.globalSelectedIndex}
								onclick={() => notePage.insertGlobalBlockLink(block)}
							>
								<div>
									<span
										style="font-size: 0.9em; opacity: 0.9; display: -webkit-box; -webkit-line-clamp: 2; -webkit-box-orient: vertical; overflow: hidden; text-align: left;"
									>
										{block.text}
									</span>
									<span
										style="font-size: 0.7em; opacity: 0.6; display: block; margin-top: 2px; text-align: left;"
									>
										From: {block.sourceNoteTitle}
									</span>
								</div>
							</button>
						</li>
					{/each}
				</ul>
			{:else}
				<p class="empty-state">
					{notePage.globalBlocks.length > 0 ? 'No matching blocks found.' : 'Loading blocks...'}
				</p>
			{/if}
		</div>

		<div class="dialog-actions">
			<button class="secondary" onclick={() => notePage.globalSearchDialog?.close()}>Cancel</button>
		</div>
	</div>
</dialog>

<dialog
	bind:this={notePage.previewNoteDialog}
	class="preview-dialog"
	onclose={() => {
		notePage.previewNoteTarget = null;
	}}
>
	{#if notePage.previewNoteTarget}
		<div class="preview-layout">
			<div class="preview-main">
				<div class="preview-header">
					<h2>{notePage.previewNoteTarget.title}</h2>
					<div class="preview-meta">
						{#if notePage.previewNoteTarget.tags.length > 0}
							<span>{notePage.previewNoteTarget.tags.join(', ')}</span>
						{/if}
					</div>
				</div>
				<div class="preview-content-scroll">
					<div
						bind:this={notePage.previewNoteContainer}
						class="vditor-reset"
						style="padding: 2rem; min-height: 100%;"
					></div>
				</div>
			</div>
			<div class="preview-sidebar">
				<button class="icon-btn" onclick={() => notePage.previewNoteDialog?.close()} title="Close Preview">
					<svg
						viewBox="0 0 24 24"
						width="24"
						height="24"
						stroke="currentColor"
						stroke-width="2"
						fill="none"
						stroke-linecap="round"
						stroke-linejoin="round"
						><line x1="18" y1="6" x2="6" y2="18"></line><line x1="6" y1="6" x2="18" y2="18"
						></line></svg
					>
				</button>
				<button class="icon-btn" onclick={notePage.expandPreviewNoteDirect} title="Expand Note">
					<svg
						viewBox="0 0 24 24"
						width="24"
						height="24"
						stroke="currentColor"
						stroke-width="2"
						fill="none"
						stroke-linecap="round"
						stroke-linejoin="round"
						><path d="M15 3h6v6"></path><path d="M9 21H3v-6"></path><path d="M21 3l-7 7"
						></path><path d="M3 21l7-7"></path></svg
					>
				</button>
			</div>
		</div>
	{/if}
</dialog>

<dialog bind:this={notePage.navigationWarningDialog} class="dialog math-dialog" onclose={notePage.cancelNavigation}>
	<div class="dialog-content">
		<h3 style="margin-top: 0;">Unsaved Changes</h3>
		<p style="color: var(--text-secondary); margin-bottom: var(--space-6);">
			The document is currently saving. Are you sure you want to leave? Unsaved changes may be lost.
		</p>
		<div class="dialog-actions">
			<button class="secondary" onclick={notePage.cancelNavigation}>Cancel</button>
			<button class="primary" onclick={notePage.confirmNavigation}>Leave Page</button>
		</div>
	</div>
</dialog>

<dialog
	bind:this={notePage.deleteAttachedNoteDialog}
	class="dialog math-dialog"
	onclose={notePage.cancelDeleteAttachedNote}
>
	<div class="dialog-content">
		<h3 style="margin-top: 0;">Delete Attached Note</h3>
		<p style="color: var(--text-secondary); margin-bottom: var(--space-6);">
			All data and annotations in this attached note will be deleted permanently, and the note pane
			will be closed.
		</p>
		<div class="dialog-actions">
			<button class="secondary" onclick={notePage.cancelDeleteAttachedNote}>Cancel</button>
			<button class="danger" onclick={notePage.confirmDeleteAttachedNote} disabled={notePage.isBusy}
				>Delete Note</button
			>
		</div>
	</div>
</dialog>

<dialog bind:this={notePage.deleteMainNoteDialog} class="dialog math-dialog">
	<div class="dialog-content">
		<h3 style="margin-top: 0;">Delete Note</h3>
		<p style="color: var(--text-secondary); margin-bottom: var(--space-6);">
			This note will be permanently deleted. This action cannot be undone.
		</p>
		<div class="dialog-actions">
			<button class="secondary" onclick={() => notePage.deleteMainNoteDialog?.close()}>Cancel</button>
			<button
				class="danger"
				onclick={() => {
					notePage.deleteMainNoteDialog?.close();
					notePage.deleteNote();
				}}
				disabled={notePage.isBusy}>Delete Note</button
			>
		</div>
	</div>
</dialog>

<dialog bind:this={notePage.detachPdfDialog} class="dialog math-dialog">
	<div class="dialog-content">
		<h3 style="margin-top: 0;">Close PDF</h3>
		<p style="color: var(--text-secondary); margin-bottom: var(--space-6);">
			The PDF will be detached from this note. You can re-attach it at any time.
		</p>
		<div class="dialog-actions">
			<button class="secondary" onclick={() => notePage.detachPdfDialog?.close()}>Cancel</button>
			<button class="danger" onclick={notePage.confirmDetachPdf} disabled={notePage.isBusy}>Close PDF</button>
		</div>
	</div>
</dialog>

<dialog
	bind:this={notePage.attachPdfDialog}
	class="pdf-attach-dialog"
	onclose={() => {
		notePage.pdfSearchQuery = '';
		notePage.pdfSelectedIndex = 0;
	}}
>
	<div class="dialog-content">
		<h3>Attach a file</h3>
		<p class="dialog-subtitle">Select a PDF from your workspace or upload a new one.</p>

		<input
			class="link-search-input"
			bind:value={notePage.pdfSearchQuery}
			oninput={() => (notePage.pdfSelectedIndex = 0)}
			placeholder="Search PDFs..."
		/>

		<div class="pdf-grid-container">
			<button class="pdf-grid-upload-card" onclick={notePage.browseAndAttachPdf} disabled={notePage.isBusy}>
				<div class="upload-icon-wrapper">
					<svg
						viewBox="0 0 24 24"
						width="32"
						height="32"
						stroke="currentColor"
						stroke-width="1.5"
						fill="none"
						stroke-linecap="round"
						stroke-linejoin="round"
						><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"></path><polyline
							points="17 8 12 3 7 8"
						></polyline><line x1="12" y1="3" x2="12" y2="15"></line></svg
					>
				</div>
				<span class="upload-text">Upload new file</span>
				<span class="upload-subtext">Choose a PDF from your computer</span>
			</button>

			{#each notePage.filteredPdfs as pdf, i (pdf.id)}
				<button class="pdf-grid-card" onclick={() => notePage.attachPdf(pdf)}>
					<div class="pdf-card-icon">
						<svg
							viewBox="0 0 24 24"
							width="24"
							height="24"
							stroke="var(--accent-300)"
							stroke-width="1.5"
							fill="none"
							stroke-linecap="round"
							stroke-linejoin="round"
							><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"></path><polyline
								points="14 2 14 8 20 8"
							></polyline></svg
						>
					</div>
					<div class="pdf-card-info">
						<strong>{pdf.title}</strong>
						<span>{new Date(pdf.createdAt).toLocaleDateString()}</span>
					</div>
				</button>
			{/each}
		</div>

		<div class="dialog-actions">
			<button class="secondary" onclick={() => notePage.attachPdfDialog?.close()}>Cancel</button>
		</div>
	</div>
</dialog>

