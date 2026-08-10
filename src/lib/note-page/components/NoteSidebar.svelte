<script lang="ts">
	import type { NotePageController } from '$lib/note-page/controller.svelte';
	import { noteSidebarOpen } from '$lib/stores';
	import { formatBacklinkContext } from '$lib/backlinkContext';
	import ChatSidebarTab from './ChatSidebarTab.svelte';

	let { notePage }: { notePage: NotePageController } = $props();
</script>

			<!-- Right Sidebar -->
			{#if $noteSidebarOpen}
				<!-- svelte-ignore a11y_click_events_have_key_events -->
				<!-- svelte-ignore a11y_no_static_element_interactions -->
				<div class="sidebar-backdrop" onclick={() => ($noteSidebarOpen = false)}></div>
			{/if}
			<aside
				class="sidebar"
				class:open={$noteSidebarOpen}
				style="--sidebar-width: {notePage.sidebarWidth}px;"
			>
				<!-- svelte-ignore a11y_no_static_element_interactions -->
				<div
					class="sidebar-resizer"
					onmousedown={notePage.startSidebarResizing}
					class:resizing={notePage.isSidebarResizing}
				></div>
				<div class="sidebar-tabs">
					<button
						class:active={notePage.activeSidebarTab === 'info'}
						onclick={() => (notePage.activeSidebarTab = 'info')}>Info</button
					>
					<button
						class:active={notePage.activeSidebarTab === 'chat'}
						onclick={() => (notePage.activeSidebarTab = 'chat')}>Chat</button
					>
					<button
						class:active={notePage.activeSidebarTab === 'versions'}
						onclick={() => {
							notePage.activeSidebarTab = 'versions';
							notePage.fetchNoteHistory();
						}}>History</button
					>
				</div>

				<div class="sidebar-content">
					{#if notePage.activeSidebarTab === 'info'}
						<div class="sidebar-section">
							<h3>Tags</h3>
							<input
								class="tag-input"
								bind:value={notePage.draftTags}
								oninput={notePage.triggerAutoSave}
								placeholder="comma,separated,tags"
								onblur={notePage.fetchRelatedNotes}
							/>
						</div>

						<div class="sidebar-section">
							<h3>Related Notes</h3>
							{#if notePage.relatedNotes.length > 0}
								<ul class="related-list">
									{#each notePage.relatedNotes as rel, i (rel.id + '_' + i)}
										<li><a href="/notes/{encodeURIComponent(rel.id)}">{rel.title}</a></li>
									{/each}
								</ul>
							{:else}
								<p class="empty-state">No related notes found.</p>
							{/if}
						</div>

						<div class="sidebar-section">
							<h3>Backlinks</h3>
							{#if notePage.note && notePage.note.backlinks && notePage.note.backlinks.length > 0}
								<ul class="related-list">
									{#each notePage.note.backlinks as link, i (link.sourceId + '_' + (link.targetBlock || '') + '_' + i)}
										<li>
											<a href="/notes/{encodeURIComponent(link.sourceId)}">
												<strong>{link.sourceTitle}</strong>
												{#if link.targetBlock}
													<span style="opacity: 0.7; font-size: 0.8em;">#{link.targetBlock}</span>
												{/if}
											</a>
											<p
												class="context-excerpt"
												style="font-size: 0.75rem; color: var(--text-secondary); margin-top: 0.25rem; line-height: 1.4;"
											>
												{@html formatBacklinkContext(link.contextExcerpt)}
											</p>
										</li>
									{/each}
								</ul>
							{:else}
								<p class="empty-state">No backlinks yet.</p>
							{/if}
						</div>

						{#if notePage.workingDocType === 'tex'}
							<div class="sidebar-section latex-preview-section" aria-live="polite">
								<h3>LaTeX preview</h3>
								<div class="latex-preview-controls">
									<button
										class="secondary latex-auto-toggle"
										onclick={() => (notePage.texAutoCompile = !notePage.texAutoCompile)}
									>
										Auto: {notePage.texAutoCompile ? 'on' : 'off'}
									</button>
									<button
										class="primary"
										disabled={notePage.texCompiling}
										onclick={() => void notePage.compileTex({ manual: true })}
									>
										{notePage.texCompiling ? 'Compiling…' : 'Compile to PDF'}
									</button>
								</div>
								<div class="latex-preview-state" class:error={notePage.texPreviewStatus === 'error'}>
									<span
										class="latex-status-dot"
										class:active={notePage.texPreviewStatus === 'current'}
										class:pending={notePage.texPreviewStatus === 'pending' ||
											notePage.texPreviewStatus === 'compiling'}
									></span>
									{#if notePage.latexDownloadMsg}
										{notePage.latexDownloadMsg}
									{:else if notePage.texCompiling}
										Compiling preview…
									{:else if notePage.texPreviewStatus === 'pending'}
										Preview pending…
									{:else if notePage.texPreviewStatus === 'current'}
										Preview current
									{:else if notePage.texPreviewStatus === 'error'}
										Preview error
									{:else}
										Preview idle
									{/if}
								</div>
								{#if notePage.texCompileError}
									<p class="latex-error">{notePage.texCompileError}</p>
								{/if}
								{#if notePage.texDiagnostics.length > 0}
									<ul class="latex-diagnostics">
										{#each notePage.texDiagnostics as diagnostic}
											<li>
												<span>{diagnostic.line ? `Line ${diagnostic.line}: ` : ''}</span
												>{diagnostic.message}
											</li>
										{/each}
									</ul>
								{/if}
							</div>
						{/if}
					{:else if notePage.activeSidebarTab === 'chat'}
						<ChatSidebarTab {notePage} />
					{:else if notePage.activeSidebarTab === 'versions'}
						<div class="versions-container">
							{#if notePage.isBusy && notePage.noteHistory.length === 0}
								<p class="empty-state">Loading history...</p>
							{:else if notePage.noteHistory.length === 0}
								<p class="empty-state">No history found.</p>
							{:else}
								<ul class="history-list">
									{#each notePage.noteHistory as commit (commit.hash)}
										<li>
											<div class="commit-header">
												<strong>{commit.message}</strong>
												<span class="commit-date"
													>{new Date(commit.timestamp).toLocaleString()}</span
												>
											</div>
											<div class="commit-actions">
												<button class="secondary" onclick={() => notePage.previewVersion(commit.hash)}
													>Preview</button
												>
												<button class="secondary" onclick={() => notePage.restoreVersion(commit.hash)}
													>Restore</button
												>
											</div>
										</li>
									{/each}
								</ul>
							{/if}
						</div>
					{/if}
				</div>
			</aside>
