<script lang="ts">
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { open as openFileDialog } from '@tauri-apps/plugin-dialog';
import { goto, beforeNavigate } from '$app/navigation';
import { base, resolve } from '$app/paths';
import { page } from '$app/state';
import type {
		NoteDocument,
		SearchResponse,
		NoteSummary,
		PdfAnnotation,
		GitCommit,
		ChatMessage
	} from '$lib/types';
import { onMount, onDestroy, tick } from 'svelte';
import { noteOpened, noteClosed } from '$lib/llamaWarm';
import { chatSidebarShortcut, showSidebarToggle, noteSidebarOpen } from '$lib/stores';
import { shortcutMatches } from '$lib/keyboardShortcut';
import { formatBacklinkContext } from '$lib/backlinkContext';
import { theme } from '$lib/theme';
import type Vditor from 'vditor';
import 'mathlive/fonts.css';
import ChatToolIndicator from '$lib/components/ChatToolIndicator.svelte';
import { hideThinkingContent } from '$lib/chatContent';
import {
		composeNoteStreamPreviewWithStatus,
		locateNoteStreamTarget
	} from '$lib/noteStreamPreview';
import { resolveActiveAiTarget } from '$lib/aiTarget';
import {
		canApplyReconciledNote,
		editorNeedsAuthoritativeBody,
		hasNoteMutation
	} from '$lib/noteMutation';
import { marked } from 'marked';
import DOMPurify from 'dompurify';
import { vditorI18n } from '$lib/vditorI18n';
import { createNotePageController } from '$lib/note-page/controller.svelte';

const notePage = createNotePageController();
</script>


<svelte:head>
	<title>{notePage.note ? notePage.note.title : 'Myelin'}</title>
	<style>
		/* Bruteforce hide the left scrollbar in split view to prevent Svelte scoping issues */
		.vditor-sv::-webkit-scrollbar {
			display: none !important;
			width: 0 !important;
			background: transparent !important;
		}
		.vditor-sv {
			scrollbar-width: none !important;
			-ms-overflow-style: none !important;
		}
	</style>
</svelte:head>

<div
	class="editor-shell"
	class:has-attached-file={!!notePage.note?.sourcePdf || (notePage.isSourceMaterial && !!notePage.activeSourceBytes)}
	class:resizing={notePage.isResizing || notePage.isSidebarResizing}
>
	<header class="editor-header">
		<div class="header-copy">
			<button class="back-link" onclick={notePage.goBack} aria-label="Go back" title="Go back">
				<svg
					viewBox="0 0 24 24"
					width="20"
					height="20"
					stroke="currentColor"
					stroke-width="2"
					fill="none"
					stroke-linecap="round"
					stroke-linejoin="round"
				>
					<line x1="19" y1="12" x2="5" y2="12"></line>
					<polyline points="12 19 5 12 12 5"></polyline>
				</svg>
			</button>
			{#if notePage.message}
				<p class="status">{notePage.message}</p>
			{/if}
			<input
				class="title-input"
				bind:value={notePage.draftTitle}
				oninput={notePage.triggerAutoSave}
				placeholder="Note title"
			/>

			<div class="save-indicator" class:saving={notePage.saveStatus === 'saving'}>
				{#if notePage.saveStatus === 'saving'}
					<svg
						class="spinner"
						viewBox="0 0 24 24"
						width="14"
						height="14"
						stroke="currentColor"
						stroke-width="2"
						fill="none"
						stroke-linecap="round"
						stroke-linejoin="round"
						><circle cx="12" cy="12" r="10"></circle><path d="M12 6v6l4 2"></path></svg
					> Saving
				{:else if notePage.saveStatus === 'unsaved'}
					<span class="dot"></span> Unsaved
				{:else}
					<svg
						viewBox="0 0 24 24"
						width="14"
						height="14"
						stroke="currentColor"
						stroke-width="2"
						fill="none"
						stroke-linecap="round"
						stroke-linejoin="round"><polyline points="20 6 9 17 4 12"></polyline></svg
					> Saved
				{/if}
			</div>
		</div>
	</header>

	{#if notePage.isLoadingNote}
		<div class="note-loading" role="status" aria-live="polite">
			<svg
				class="note-loading-spinner"
				viewBox="0 0 24 24"
				width="18"
				height="18"
				fill="none"
				stroke="currentColor"
				stroke-width="2"
				stroke-linecap="round"
			>
				<path d="M21 12a9 9 0 1 1-6.219-8.56" />
			</svg>
			<span>Opening note…</span>
		</div>
	{/if}

	<div
		class="main-layout"
		class:split-layout={notePage.activeSourceBytes !== null && notePage.showAttachedNote}
		bind:this={notePage.mainLayoutEl}
	>
		{#if notePage.activeSourceBytes}
			<section
				class="pdf-pane"
				class:tex-pane={notePage.workingDocType === 'tex'}
				style="position: relative; width: {!notePage.showAttachedNote ? '100%' : `${notePage.splitRatio}%`}"
			>
				{#if notePage.sectionCache}
					<div
						class="section-cache-overlay"
						class:section-cache-complete={notePage.sectionCache.finished}
						class:section-cache-failed={notePage.sectionCache.finished && notePage.sectionCache.failed > 0}
						role="status"
						aria-live="polite"
					>
						<div class="section-cache-bar">
							<div
								class="section-cache-fill"
								style="width: {(notePage.sectionCache.done / notePage.sectionCache.total) * 100}%"
							></div>
						</div>
						<span class="section-cache-label">
							{#if notePage.sectionCache.finished && notePage.sectionCache.failed > 0}
								Prepared {notePage.sectionCache.sectionDone}/{notePage.sectionCache.sectionTotal} sections;
								{notePage.sectionCache.failed} section caches failed.
								{notePage.sectionCache.failedDetails.length > 0 ? ` Failed: ${notePage.sectionCache.failedDetails.join(', ')}.` : ''}
								Took {notePage.formatSectionCacheDuration(notePage.sectionCache.elapsedMs ?? 0)}. Reopen to retry.
							{:else if notePage.sectionCache.finished}
								Prepared {notePage.sectionCache.sectionDone}/{notePage.sectionCache.sectionTotal} sections for Chat + Write in
								{notePage.formatSectionCacheDuration(notePage.sectionCache.elapsedMs ?? 0)}
							{:else}
								Preparing {notePage.sectionCache.done}/{notePage.sectionCache.total} shared section caches
								{notePage.sectionCache.label ? ` — ${notePage.sectionCache.label}` : ''}
								{notePage.sectionCache.profile === 'shared' ? ' · Chat + Write' : notePage.sectionCache.profile ? ` · ${notePage.sectionCache.profile}` : ''}…
							{/if}
						</span>
					</div>
				{/if}
				{#if notePage.workingDocType === 'tex'}
					<div class="tex-pane-badge">PDF preview</div>
				{/if}
				{#if notePage.sourceMaterialType === 'pdf' && notePage.PdfViewerComponent}
					<svelte:component this={notePage.PdfViewerComponent}
						pdfBytes={notePage.activeSourceBytes}
						annotations={notePage.note?.annotations || []}
						onQuote={notePage.handlePdfQuote}
						onAnnotationsChange={notePage.handleAnnotationsChange}
						onImageExtract={notePage.handleImageExtract}
						onTextExtracted={notePage.handlePdfTextExtracted}
						onActiveSection={notePage.handleActiveSectionChange}
						onSectionsReady={notePage.handleSectionsReady}
						onClosePdf={notePage.workingDocType === 'tex'
							? notePage.closeTexPreview
							: notePage.activeSourceBytes !== null && notePage.showAttachedNote
								? notePage.requestDetachPdf
								: undefined}
						onAttachNote={() => void notePage.openAttachedNote()}
						showAttachButton={!notePage.showAttachedNote}
					/>
				{:else if notePage.sourceMaterialType === 'pdf'}
					<div class="viewer-loading">Loading PDF viewer…</div>
				{:else if notePage.sourceMaterialType === 'epub' && notePage.EpubViewerComponent}
					<svelte:component this={notePage.EpubViewerComponent}
						epubBytes={notePage.activeSourceBytes}
						onActiveSection={notePage.handleActiveSectionChange}
						onSectionsReady={notePage.handleSectionsReady}
					/>
					{#if !notePage.showAttachedNote}
						<button
							style="position: absolute; top: 10px; right: 10px;"
							class="primary"
							onclick={() => {
								notePage.showAttachedNote = true;
								setTimeout(() => notePage.initVditor(), 100);
							}}>Attach Note</button
						>
					{/if}
				{:else if notePage.sourceMaterialType === 'epub'}
					<div class="viewer-loading">Loading EPUB viewer…</div>
				{:else if notePage.sourceMaterialType === 'html' && notePage.HtmlViewerComponent}
						<svelte:component this={notePage.HtmlViewerComponent}
							htmlBytes={notePage.activeSourceBytes}
						onActiveSection={notePage.handleActiveSectionChange}
							onSectionsReady={notePage.handleSectionsReady}
						/>
					{#if !notePage.showAttachedNote}
						<button
							style="position: absolute; top: 10px; right: 10px;"
							class="primary"
							onclick={() => {
								notePage.showAttachedNote = true;
								setTimeout(() => notePage.initVditor(), 100);
							}}>Attach Note</button
						>
					{/if}
				{:else if notePage.sourceMaterialType === 'html'}
					<div class="viewer-loading">Loading document viewer…</div>
				{/if}
				{#if notePage.sourceMaterialType === 'pdf' && (notePage.pdfIngestionStatus === 'indexing' || notePage.pdfIngestionStatus === 'empty' || notePage.pdfIngestionStatus === 'failed')}
					<div
						class="pdf-index-status"
						class:error={notePage.pdfIngestionStatus === 'failed'}
						title={notePage.pdfIngestionError ?? undefined}
					>
						{notePage.pdfIngestionStatus === 'indexing'
							? 'Indexing PDF for AI…'
							: notePage.pdfIngestionStatus === 'empty'
								? 'No selectable PDF text to index'
								: 'PDF indexing failed'}
					</div>
				{/if}
			</section>
			{#if notePage.showAttachedNote}
				<!-- svelte-ignore a11y_no_static_element_interactions -->
				<div class="resizer" onmousedown={notePage.startResizing} class:resizing={notePage.isResizing}></div>
			{/if}
		{/if}

		<!-- Main Content Area -->
		{#if notePage.shouldRenderEditor}
			<section
				class="main-pane"
				class:tex-pane={notePage.workingDocType === 'tex'}
				style={notePage.activeSourceBytes ? `width: ${100 - notePage.splitRatio}%` : ''}
			>
				<div class="content-area" style="position: relative;">
					{#if notePage.workingDocType === 'md'}
						<!-- svelte-ignore a11y_click_events_have_key_events -->
						<!-- svelte-ignore a11y_no_static_element_interactions -->
						<div
							bind:this={notePage.vditorContainer}
							class="vditor-wrapper"
							class:tools-loading={!notePage.toolsReady || notePage.vditorLoading}
							class:toolbar-expanded={notePage.toolbarExpanded}
							class:has-pdf-note={!!notePage.activeSourceBytes || (!notePage.isSourceMaterial && !!notePage.note)}
							onclickcapture={notePage.handleVditorClick}
							onkeydowncapture={notePage.handleVditorKeydownCapture}
							onkeyupcapture={notePage.handleVditorKeyupCapture}
							onwheelcapture={(e) => {
								if (e.ctrlKey || e.metaKey) {
									e.preventDefault();
									e.stopPropagation();
								}
							}}
						>
							{#if !notePage.toolsReady || notePage.vditorLoading}
								<div class="tools-loading-overlay" aria-live="polite">Loading editing tools…</div>
							{/if}
						</div>
						<div class="fullscreen-indicator">
							Press <span>{notePage.fullscreenShortcut}</span> to toggle
						</div>
					{:else if notePage.workingDocType === 'tex' && notePage.TexEditorComponent}
						<svelte:component this={notePage.TexEditorComponent}
							bind:this={notePage.texEditorInstance}
							value={notePage.draftBody}
							onInput={(val: string) => {
								notePage.lastTexBody = val;
								notePage.texRevision += 1;
								if (notePage.texAutoCompile && notePage.texCacheWarmed) notePage.texPreviewStatus = 'pending';
								notePage.draftBody = val;
								notePage.triggerAutoSave();
							}}
							diagnostics={notePage.texDiagnostics}
							onPickImage={notePage.pickLatexImage}
							onAiTargetChange={notePage.captureExternalTarget}
							busy={notePage.isBusy}
						/>
					{:else if notePage.workingDocType === 'tex'}
						<div class="editor-loading">Loading LaTeX editor…</div>
					{:else if notePage.workingDocType === 'ipynb' && notePage.IpynbEditorComponent}
						<svelte:component this={notePage.IpynbEditorComponent}
							bind:this={notePage.ipynbEditorInstance}
							value={notePage.draftBody}
							onInput={(val: string) => {
								notePage.draftBody = val;
								notePage.triggerAutoSave();
							}}
							onAiTargetChange={notePage.captureExternalTarget}
						/>
					{:else}
						<div class="editor-loading">Loading notebook editor…</div>
					{/if}

					{#if notePage.isSourceMaterial && notePage.activeSourceBytes && notePage.showAttachedNote}
						<div
							class="toolbar-close-note-container"
							style={notePage.toolbarNeedsToggle ? 'right: 50px;' : 'right: 12px;'}
						>
							<button
								class="toolbar-close-note-btn"
								onclick={notePage.requestDeleteAttachedNote}
								disabled={notePage.isBusy}
								title="Delete attached note and close pane"
							>
								Close Note
							</button>
						</div>
					{/if}
					<div class="toolbar-note-actions-container">
						{#if notePage.toolbarNeedsToggle}
							<button
								class="toolbar-overlay-toggle"
								class:expanded={notePage.toolbarExpanded}
								onclick={() => (notePage.toolbarExpanded = !notePage.toolbarExpanded)}
								aria-label="Toggle toolbar"
							>
								<svg
									viewBox="0 0 24 24"
									width="16"
									height="16"
									stroke="currentColor"
									stroke-width="2"
									fill="none"
									stroke-linecap="round"
									stroke-linejoin="round"
								>
									<polyline points="6 9 12 15 18 9"></polyline>
								</svg>
							</button>
						{/if}
						{#if notePage.activeSourceBytes !== null && notePage.showAttachedNote}
							<button
								class="toolbar-overlay-toggle"
								onclick={notePage.requestDeleteMainNote}
								aria-label="Delete Note"
								title="Delete Note"
							>
								<svg
									viewBox="0 0 24 24"
									width="14"
									height="14"
									stroke="var(--danger, #ef4444)"
									stroke-width="2"
									fill="none"
									stroke-linecap="round"
									stroke-linejoin="round"
								>
									<line x1="18" y1="6" x2="6" y2="18"></line>
									<line x1="6" y1="6" x2="18" y2="18"></line>
								</svg>
							</button>
						{/if}
					</div>
				</div>
			</section>

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
						<div class="chat-container">
							<div class="chat-messages" bind:this={notePage.chatMessagesEl} onscroll={notePage.handleChatScroll}>
								{#if notePage.chatMessages.length === 0}
									<p class="empty-state">Ask me anything about this note or your library!</p>
								{:else}
									{#each notePage.chatMessages as msg, i}
										{@const visibleContent =
											msg.role === 'assistant' ? hideThinkingContent(msg.content) : msg.content}
										{#if msg.role === 'user' || visibleContent || (msg.tools && msg.tools.length > 0) || (msg.isApprovalRequest && msg.approvalStatus !== 'approved') || msg.isStreaming || msg.error}
											<div
												class="chat-message {msg.role}"
												class:tool-only={!msg.content &&
													((msg.tools && msg.tools.length > 0) || msg.isApprovalRequest)}
											>
												<div class="chat-bubble" class:error={msg.error}>
													{#if msg.tools && msg.tools.length > 0}
														<div class="chat-tools">
															{#each msg.tools as tool}
																<ChatToolIndicator {tool} />
															{/each}
														</div>
													{/if}
													{#if msg.error}
														<span class="chat-error-text">
															{visibleContent || 'Failed to generate response.'}
														</span>
													{:else if msg.isApprovalRequest && msg.approvalStatus !== 'approved'}
														<ChatToolIndicator
															tool={{
																name:
																	(msg.approvalStatus === 'rejected'
																		? 'Rejected tool: '
																		: 'Pending tool: ') + msg.approvalTool,
																details: msg.approvalDetails || ''
															}}
														/>
													{:else if msg.role === 'assistant' && visibleContent}
														<div class="selectable-content">
															{@html notePage.renderChatContent(visibleContent)}
														</div>
													{:else if visibleContent}
														<span class="selectable-content">{visibleContent}</span>
													{/if}
													{#if msg.isStreaming && msg.startTime}
														{#if !msg.content}
															{#if msg.statusText}
																<span class="chat-progress" role="status" aria-live="polite"
																	>{msg.statusText}</span
																>
															{/if}
															<span class="chat-working" aria-label="Working"
																><span></span><span></span><span></span></span
															>
														{/if}
														<span class="chat-time-taken live"
															>{((notePage.currentTime - msg.startTime) / 1000).toFixed(1)}s</span
														>
													{:else if msg.endTime && msg.startTime}
														<span class="chat-time-taken"
															>{((msg.endTime - msg.startTime) / 1000).toFixed(1)}s</span
														>
													{/if}
												</div>
												{#if msg.role === 'user' && msg.snapshot}
													<div class="chat-msg-actions">
														<button
															class="rewind-btn"
															onclick={() => notePage.rewindToSnapshot(msg.snapshot, msg.content)}
															title="Undo — restore note and put prompt back in input">↩</button
														>
														<button
															class="rewind-btn retry"
															onclick={() => notePage.retryMessage(msg.snapshot!, msg.content)}
															title="Retry — rewind and resend this prompt">↻</button
														>
													</div>
												{/if}
												{#if msg.role === 'assistant' && visibleContent && !msg.isStreaming}
													<div class="chat-msg-actions assistant">
														<button
															class="rewind-btn copy-btn"
															onclick={() => notePage.copyMessage(i, visibleContent)}
															title="Copy response"
															aria-label="Copy response"
														>
															{#if notePage.copiedIdx === i}✓{:else}
																<svg
																	width="13"
																	height="13"
																	viewBox="0 0 24 24"
																	fill="none"
																	stroke="currentColor"
																	stroke-width="2"
																	stroke-linecap="round"
																	stroke-linejoin="round"
																	><rect x="9" y="9" width="13" height="13" rx="2" ry="2" /><path
																		d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"
																	/></svg
																>
															{/if}
														</button>
													</div>
												{/if}
											</div>
										{/if}
									{/each}
								{/if}
							</div>

							{#if notePage.chatMessages.find((m) => m.isApprovalRequest && m.approvalStatus === 'pending')}
								{@const pendingReq = notePage.chatMessages.find(
									(m) => m.isApprovalRequest && m.approvalStatus === 'pending'
								)}
								<div class="pending-approval-bar">
									<div class="pending-info">
										<span class="tool-icon">⚡</span>
										<span class="pending-text"
											>AI wants to use <strong>{pendingReq?.approvalTool}</strong></span
										>
									</div>
									<div class="pending-actions">
										<button
											class="primary"
											onclick={() => notePage.resolveApproval(pendingReq!.approvalId!, true)}>Approve</button
										>
										<button
											class="secondary"
											onclick={() => notePage.resolveApproval(pendingReq!.approvalId!, false)}>Reject</button
										>
									</div>
								</div>
							{/if}

							<div class="chat-input-area">
								{#if notePage.showDebugWindow && notePage.debugInfo}
									<div class="debug-window">
										<div class="debug-window-header">
											<span class="debug-title">AI Debug</span>
											<button
												class="debug-toggle"
												onclick={() => (notePage.showDebugWindow = false)}
												title="Close debug window">×</button
											>
										</div>
										<div class="debug-grid">
											<div class="debug-row">
												<span class="debug-label">Prompt → First token:</span>
												<span class="debug-value"
													>{notePage.debugInfo.firstChunk && notePage.debugInfo.requestStart
														? ((notePage.debugInfo.firstChunk - notePage.debugInfo.requestStart) / 1000).toFixed(2) +
															's'
														: '—'}</span
												>
											</div>
											<div class="debug-row">
												<span class="debug-label">First token → Done:</span>
												<span class="debug-value"
													>{notePage.debugInfo.done && notePage.debugInfo.firstChunk
														? ((notePage.debugInfo.done - notePage.debugInfo.firstChunk) / 1000).toFixed(2) + 's'
														: '—'}</span
												>
											</div>
											<div class="debug-row">
												<span class="debug-label">Total elapsed:</span>
												<span class="debug-value"
													>{notePage.debugInfo.done
														? ((notePage.debugInfo.done - (notePage.debugInfo.requestStart ?? 0)) / 1000).toFixed(2) +
															's'
														: notePage.debugInfo.requestStart
															? ((Date.now() - notePage.debugInfo.requestStart) / 1000).toFixed(2) +
																's (live)'
															: '—'}</span
												>
											</div>
											<div class="debug-row">
												<span class="debug-label">Prompt tokens:</span>
												<span class="debug-value">{notePage.debugInfo.promptTokens || '—'}</span>
											</div>
											<div class="debug-row">
												<span class="debug-label">Completion tokens:</span>
												<span class="debug-value">{notePage.debugInfo.completionTokens || '—'}</span>
											</div>
											<div class="debug-row">
												<span class="debug-label">Tokens/s (model):</span>
												<span class="debug-value">
													{#if notePage.debugInfo.generationStart && notePage.debugInfo.generationEnd}
														{@const genSec =
															(notePage.debugInfo.generationEnd - notePage.debugInfo.generationStart) / 1000}
														{@const fromServer = notePage.debugInfo.completionTokens > 0}
														{@const tokCount = fromServer
															? notePage.debugInfo.completionTokens
															: Math.round(notePage.debugInfo.replyChars / 4)}
														{fromServer ? '' : '~'}{(tokCount / genSec).toFixed(1)}
													{:else}
														—
													{/if}
												</span>
											</div>
										</div>
										<div class="debug-trace selectable-content" bind:this={notePage.debugTraceEl}>
											{#each notePage.debugInfo.trace as entry}
												<div class="trace-entry {entry.kind}">
													<span class="trace-time"
														>+{(
															(entry.time - (notePage.debugInfo.requestStart ?? entry.time)) /
															1000
														).toFixed(1)}s</span
													>
													{#if entry.kind === 'model_prompt' || entry.kind === 'intent_prompt'}
														<details class="trace-prompt">
															<summary
																>{entry.kind === 'intent_prompt'
																	? 'Intent classifier prompt'
																	: 'Model request prompt'}</summary
															>
															<pre>{entry.msg}</pre>
														</details>
													{:else if entry.kind === 'tool'}
														<span class="trace-msg trace-tool">🔧 {entry.msg}</span>
													{:else if entry.kind === 'tool_result'}
														<span class="trace-msg trace-tool-result">✅ {entry.msg}</span>
													{:else if entry.kind === 'config'}
														<span class="trace-msg trace-config">⚙️ {entry.msg}</span>
													{:else if entry.kind === 'error'}
														<span class="trace-msg trace-error">❌ {entry.msg}</span>
													{:else if entry.kind === 'gen'}
														<span class="trace-msg trace-gen">💬 {entry.msg}</span>
													{:else if entry.kind === 'done'}
														<span class="trace-msg trace-done">✓ {entry.msg}</span>
													{:else}
														<span class="trace-msg">{entry.msg}</span>
													{/if}
												</div>
											{/each}
										</div>
									</div>
								{/if}
								<div class="prompt-box">
									<textarea
										bind:this={notePage.chatTextareaEl}
										bind:value={notePage.chatInput}
										onkeydown={(e) => {
											if (e.key === 'Enter' && !e.shiftKey) {
												e.preventDefault();
												if (notePage.chatInput.trim() && !notePage.isChatStreaming) notePage.sendChatMessage();
											}
										}}
										oninput={(e) => {
											const target = e.target as HTMLTextAreaElement;
											target.style.height = 'auto';
											target.style.height = `${Math.min(target.scrollHeight + 2, 150)}px`;
										}}
										placeholder="Ask AI…"
										rows="1"
									></textarea>
									{#if notePage.writeTargetNotice && notePage.aiInteractionMode === 'write'}
										<div class="write-target-notice" role="status">
											Place the cursor where you want to write, or select text to rewrite. Then send
											again.
										</div>
									{/if}
									<div class="prompt-toolbar">
										<div class="interaction-mode" role="group" aria-label="AI interaction mode">
											<button
												type="button"
												class:active={notePage.aiInteractionMode === 'chat'}
												onclick={() => notePage.setAiInteractionMode('chat')}
												title="Chat: answer questions without modifying the note">Chat</button
											>
											<button
												type="button"
											class:active={notePage.aiInteractionMode === 'write'}
											onclick={() => notePage.setAiInteractionMode('write')}
												title="Write: perform the request on the open note">Write</button
											>
										</div>
										<div class="interaction-mode approval-toggle">
											<button
												type="button"
												class:active={!notePage.requireToolApproval}
												onclick={() => notePage.setToolApproval(!notePage.requireToolApproval)}
												title={notePage.requireToolApproval
													? 'Ask before each tool action — click to allow automatically'
													: 'Allow tool actions without confirmation — click to ask first'}
											>
												{notePage.requireToolApproval ? 'Ask' : 'Allow'}
											</button>
										</div>
										<button
											type="button"
											class="prompt-icon-btn"
											onclick={notePage.attachFile}
											title="Attach a file"
											aria-label="Attach a file"
										>
											<svg
												width="16"
												height="16"
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
										</button>
										{#if notePage.armedSelection}
											<button
												type="button"
												class="selection-pill"
												onclick={notePage.clearArmedSelection}
												title={notePage.armedSelection.cursor
													? `Write target: ${notePage.armedSelection.cellIndex === undefined ? 'cursor' : `cell ${notePage.armedSelection.cellIndex + 1} cursor`}. Click to clear.`
													: `Use your selection as AI context — ${notePage.armedSelection.chars} chars, ${notePage.armedSelection.words} word${notePage.armedSelection.words === 1 ? '' : 's'}. Click to clear.`}
												aria-label={notePage.armedSelection.cursor
													? 'Clear cursor target'
													: `Clear selection (${notePage.armedSelection.chars} characters)`}
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
													><path
														d="M4 7V5a1 1 0 0 1 1-1h2M17 4h2a1 1 0 0 1 1 1v2M20 17v2a1 1 0 0 1-1 1h-2M7 20H5a1 1 0 0 1-1-1v-2"
													/></svg
												>
												<span>
													{#if notePage.armedSelection.cellIndex !== undefined}
														Cell {notePage.armedSelection.cellIndex + 1} ·
													{/if}
													{notePage.armedSelection.cursor ? 'Cursor' : `${notePage.armedSelection.chars} sel`}
												</span>
												<span class="sel-x" aria-hidden="true">✕</span>
											</button>
										{/if}
										<div class="prompt-spacer"></div>
										{#if notePage.isChatStreaming}
											<button
												type="button"
												class="send-btn stop-btn"
												onclick={notePage.stopChat}
												aria-label="Stop AI"
												title="Stop AI generation"
											>
												<svg
													width="15"
													height="15"
													viewBox="0 0 24 24"
													fill="currentColor"
													aria-hidden="true"
													><rect x="6" y="6" width="12" height="12" rx="1.5" /></svg
												>
											</button>
										{:else}
											<button
												type="button"
												class="send-btn"
												onclick={() => {
													if (notePage.chatInput.trim()) notePage.sendChatMessage();
												}}
												disabled={!notePage.chatInput.trim()}
												aria-label="Send"
												title="Send (Enter)"
											>
												<svg
													width="16"
													height="16"
													viewBox="0 0 24 24"
													fill="none"
													stroke="currentColor"
													stroke-width="2.2"
													stroke-linecap="round"
													stroke-linejoin="round"
													><line x1="12" y1="19" x2="12" y2="5" /><polyline
														points="5 12 12 5 19 12"
													/></svg
												>
											</button>
										{/if}
									</div>
								</div>
							</div>
						</div>
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
		{/if}
	</div>
</div>

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

<style>
	.editor-shell {
		height: 100%;
		position: relative;
		display: grid;
		grid-template-rows: auto 1fr;
		animation: fade-in var(--duration-page) var(--ease-out);
		background: var(--bg-page);
	}

	.editor-shell.resizing,
	.editor-shell.resizing :global(.textLayer) {
		user-select: none !important;
	}

	.editor-header {
		display: flex;
		justify-content: space-between;
		align-items: center;
		padding: var(--space-4) var(--space-6) var(--space-4) var(--space-8);
		border-bottom: 1px solid var(--border-default);
		background: var(--bg-panel-blur);
		backdrop-filter: blur(var(--blur-md));
		position: relative;
		z-index: 1;
	}

	.header-copy {
		display: flex;
		align-items: center;
		gap: var(--space-4);
		flex: 1;
	}

	.back-link,
	input {
		font: inherit;
		font-family: var(--font-mono);
	}

	.back-link {
		border: 1px solid var(--border-subtle);
		border-radius: var(--radius-sm);
		background: transparent;
		color: var(--text-secondary);
		padding: 0.5rem;
		display: flex;
		align-items: center;
		justify-content: center;
		cursor: pointer;
		transition: all var(--duration-fast);
	}
	.back-link:hover {
		color: var(--text-primary);
		border-color: var(--neutral-600);
	}

	.note-loading {
		position: absolute;
		inset: 4.5rem 0 0;
		z-index: 30;
		display: flex;
		align-items: center;
		justify-content: center;
		gap: var(--space-3);
		background: color-mix(in srgb, var(--bg-page) 88%, transparent);
		color: var(--text-secondary);
		font-family: var(--font-mono);
		font-size: 0.75rem;
		text-transform: uppercase;
		letter-spacing: 0.08em;
		pointer-events: none;
	}

	.note-loading-spinner {
		animation: spin 1s linear infinite;
	}

	.status {
		margin: 0;
		font-size: 0.75rem;
		text-transform: uppercase;
		letter-spacing: 0.08em;
		color: var(--text-secondary);
		width: max-content;
	}

	.title-input {
		font-size: 1.5rem;
		font-weight: 600;
		color: var(--text-hero);
		background: transparent;
		border: 1px solid transparent;
		padding: 0.25rem 0.5rem;
		font-family: var(--font-sans);
		flex: 1;
		min-width: 0;
		max-width: none;
	}
	.title-input:hover,
	.title-input:focus {
		border-color: var(--border-subtle);
		background: var(--bg-panel);
	}

	.save-indicator {
		display: flex;
		align-items: center;
		gap: 0.375rem;
		font-size: 0.75rem;
		color: var(--text-secondary);
		font-family: var(--font-mono);
		text-transform: uppercase;
		letter-spacing: 0.05em;
		padding: 0.25rem 0.5rem;
		margin-left: auto;
	}

	.save-indicator.saving {
		color: var(--accent-100);
	}

	.save-indicator .dot {
		width: 6px;
		height: 6px;
		border-radius: 50%;
		background: var(--neutral-400);
	}

	.spinner {
		animation: spin 1s linear infinite;
	}

	@keyframes spin {
		100% {
			transform: rotate(360deg);
		}
	}
	button:disabled {
		opacity: 0.6;
		cursor: not-allowed;
	}

	.main-layout {
		flex: 1;
		min-height: 0;
		position: relative;
		display: flex;
		overflow: hidden;
		z-index: 20; /* Ensures tooltips render above the header's stacking context */
	}

	.main-layout.split-layout {
		overflow-x: auto;
	}

	.pdf-pane {
		min-width: 26rem;
	}

	.section-cache-overlay {
		position: absolute;
		/* Keep it below the PDF toolbar, which occupies the first 48px of the pane. */
		top: 3.5rem;
		left: 50%;
		transform: translateX(-50%);
		z-index: 9;
		display: flex;
		align-items: center;
		gap: 0.6rem;
		width: min(26rem, calc(100% - 3rem));
		padding: 0.45rem 0.7rem;
		border: 1px solid var(--border-default);
		border-radius: var(--radius-sm);
		background: var(--bg-panel);
		box-shadow: var(--shadow-md, 0 2px 8px rgba(0, 0, 0, 0.15));
		pointer-events: none;
	}

	.section-cache-bar {
		flex: 0 0 6.5rem;
		height: 0.4rem;
		border-radius: 0.2rem;
		background: var(--border-default);
		overflow: hidden;
	}

	.section-cache-fill {
		height: 100%;
		background: var(--accent, #4f7cff);
		transition: width 0.25s ease;
	}

	.section-cache-complete .section-cache-fill {
		background: var(--success, #42a66b);
	}

	.section-cache-failed .section-cache-fill {
		background: var(--danger, #d45b5b);
	}

	.section-cache-label {
		flex: 1;
		color: var(--text-secondary);
		font-size: 0.72rem;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.pdf-index-status {
		position: absolute;
		right: 0.75rem;
		bottom: 0.75rem;
		z-index: 8;
		padding: 0.35rem 0.55rem;
		border: 1px solid var(--border-default);
		border-radius: var(--radius-sm);
		background: var(--bg-panel);
		color: var(--text-secondary);
		font-size: 0.72rem;
		pointer-events: none;
	}

	.pdf-index-status.error {
		color: var(--danger-text);
		border-color: var(--danger-border);
	}

	/* In the .tex split (PDF preview + editor), let both panes shrink with the
	   window. Otherwise main-pane's 800px min overflows the overflow:hidden layout
	   on small windows, pushing the editor's horizontal scrollbar off-screen and
	   clipping long lines. With min-width:0 the inner editor scroller handles them. */
	.main-layout .main-pane.tex-pane {
		min-width: 0;
	}
	/* Keep enough width for the PDF toolbar so its buttons don't crowd/overlap,
	   but small enough that the editor still gets room to shrink + scroll. */
	.main-layout .pdf-pane.tex-pane {
		min-width: 15rem;
	}

	/* Floating label over the compiled-PDF pane (tex split) explaining the layout. */
	.tex-pane-badge {
		position: absolute;
		top: 8px;
		left: 50%;
		transform: translateX(-50%);
		z-index: 6;
		pointer-events: none;
		max-width: 92%;
		text-align: center;
		background: var(--bg-panel);
		color: var(--text-secondary);
		border: 1px solid var(--border-default);
		border-radius: 6px;
		padding: 3px 10px;
		font-size: 0.72rem;
		box-shadow: 0 2px 8px var(--shadow-color, rgba(0, 0, 0, 0.15));
	}

	.resizer {
		width: 10px;
		flex: 0 0 10px;
		cursor: col-resize;
		position: relative;
		background: linear-gradient(
			90deg,
			transparent 0,
			transparent 3px,
			var(--border-subtle) 3px,
			var(--border-subtle) 7px,
			transparent 7px
		);
		transition: background 0.2s ease;
	}

	.resizer:hover,
	.resizer.resizing {
		background: linear-gradient(
			90deg,
			transparent 0,
			transparent 2px,
			var(--accent-100) 2px,
			var(--accent-100) 8px,
			transparent 8px
		);
	}

	.sidebar-backdrop {
		display: none;
	}

	/* Main Pane */
	.main-pane {
		flex: 1;
		min-width: 0;
		display: flex;
		flex-direction: column;
		background: var(--bg-page);
		align-items: stretch;
		min-height: 0;
		overflow-y: auto; /* Make main pane the scroll container */
		overflow-x: hidden;
	}

	.danger {
		border: 1px solid var(--danger-border);
		background: var(--danger-bg);
		color: var(--danger-text);
	}

	.danger:hover:not(:disabled) {
		background: var(--danger-bg-strong);
		color: var(--danger-text);
	}

	.content-area {
		width: 100%;
		min-width: 0;
		display: flex;
		flex-direction: column;
		flex: 1;
		min-height: 0;
	}

	.vditor-wrapper {
		position: relative;
		border: none !important;
		flex: 1;
		min-width: 0;
		min-height: 0;
	}

	.tools-loading-overlay {
		position: absolute;
		inset: 0;
		z-index: 20;
		display: flex;
		align-items: center;
		justify-content: center;
		background: color-mix(in srgb, var(--bg-primary) 88%, transparent);
		color: var(--text-muted);
		font-size: 0.8rem;
		pointer-events: auto;
		cursor: wait;
	}

	.vditor-wrapper.tools-loading :global(.vditor-toolbar) {
		opacity: 0.35;
		filter: grayscale(1);
		pointer-events: none;
	}

	:global(.vditor) {
		height: 100% !important;
	}

	:global(.vditor-reset) {
		padding-top: var(--space-6) !important;
	}

	/* Vditor ships light Markdown table colors that otherwise override the app
	   theme inside the editor and preview panes. Keep normal content wrapping,
	   but let wide tables scroll instead of forcing their cells to wrap. */
	:global(.vditor-reset table) {
		display: block !important;
		width: max-content !important;
		min-width: 0 !important;
		max-width: 100% !important;
		overflow: auto !important;
		white-space: nowrap !important;
		border: 1px solid var(--border-default) !important;
		border-collapse: collapse !important;
		background: var(--bg-panel) !important;
		color: var(--text-primary) !important;
	}
	:global(.vditor-reset table tr) {
		background: var(--bg-panel) !important;
		border-top: 1px solid var(--border-default) !important;
	}
	:global(.vditor-reset table tbody tr:nth-child(2n)) {
		background: var(--overlay-faint) !important;
	}
	:global(.vditor-reset table td),
	:global(.vditor-reset table th) {
		border: 1px solid var(--border-default) !important;
		color: var(--text-primary) !important;
		background: transparent !important;
		white-space: nowrap !important;
		word-break: normal !important;
		overflow-wrap: normal !important;
		max-width: none !important;
	}
	:global(.vditor-reset table th) {
		background: var(--bg-code) !important;
		font-weight: 600 !important;
	}

	.toolbar-notePage.note-actions-container {
		position: absolute;
		top: 0;
		right: var(--space-6);
		height: 48px;
		display: flex;
		align-items: center;
		justify-content: center;
		gap: var(--space-2);
		z-index: 40;
	}

	.toolbar-close-notePage.note-btn {
		pointer-events: auto;
		border: 1px solid var(--border-subtle);
		background: var(--bg-panel);
		color: currentColor;
		border-radius: var(--radius-sm);
		padding: 0.4rem 0.75rem;
		height: 32px;
		font-size: 0.82rem;
		font-family: var(--font-mono);
		line-height: 1;
		white-space: nowrap;
	}

	.toolbar-close-notePage.note-btn:hover:not(:disabled) {
		background: var(--danger-bg-strong);
		color: var(--danger-text);
	}

	.toolbar-attach-pdf-btn {
		pointer-events: auto;
		display: flex;
		align-items: center;
		gap: 0.375rem;
		border: 1px solid var(--border-subtle);
		background: var(--bg-panel);
		color: var(--text-secondary);
		border-radius: var(--radius-sm);
		padding: 0.4rem 0.75rem;
		height: 32px;
		font-size: 0.82rem;
		font-family: var(--font-mono);
		line-height: 1;
		white-space: nowrap;
		cursor: pointer;
		transition: all var(--duration-fast);
	}

	.toolbar-attach-pdf-btn:hover:not(:disabled) {
		border-color: var(--accent-200);
		color: var(--accent-100);
	}

	.toolbar-overlay-toggle {
		width: 28px;
		height: 28px;
		display: flex;
		align-items: center;
		justify-content: center;
		background: var(--bg-surface);
		border: 1px solid var(--border-subtle);
		border-radius: var(--radius-sm);
		color: var(--text-secondary);
		cursor: pointer;
		transition: all 0.2s;
	}
	.toolbar-overlay-toggle:hover {
		color: var(--text-primary);
		background: var(--hover-overlay);
	}
	.toolbar-overlay-toggle.expanded svg {
		transform: rotate(180deg);
	}

	:global(.vditor-wrapper:not(.toolbar-expanded) .vditor-toolbar) {
		max-height: 48px;
		overflow: visible !important;
	}

	/* Force all upward-facing tooltips (__n, __ne, __nw) to point downwards vertically */
	:global(.vditor-toolbar .vditor-tooltipped__n::after),
	:global(.vditor-toolbar .vditor-tooltipped__ne::after),
	:global(.vditor-toolbar .vditor-tooltipped__nw::after) {
		bottom: auto !important;
		top: 100% !important;
		margin-bottom: 0 !important;
		margin-top: 5px !important;
	}

	:global(.vditor-toolbar .vditor-tooltipped__n::before),
	:global(.vditor-toolbar .vditor-tooltipped__ne::before),
	:global(.vditor-toolbar .vditor-tooltipped__nw::before) {
		top: auto !important;
		bottom: -5px !important;
		border-top-color: transparent !important;
		border-bottom-color: var(--neutral-800) !important;
	}

	:global(.vditor) {
		border: none !important;
		overflow: visible !important;
		min-width: 0 !important;
		height: 100% !important;
		display: flex !important;
		flex-direction: column !important;
		--panel-background-color: var(--bg-page) !important;
		--textarea-background-color: var(--bg-page) !important;
		--toolbar-background-color: var(--bg-panel-blur) !important;
	}

	:global(.vditor-content) {
		display: flex !important;
		flex-direction: column !important;
		align-items: stretch !important;
		width: 100% !important;
		min-width: 0 !important;
		background: var(--bg-page) !important;
		flex: 1 !important;
		min-height: 0 !important;
		overflow: hidden !important;
	}

	:global(.vditor-ir),
	:global(.vditor-sv),
	:global(.vditor-preview) {
		width: 100% !important;
		box-sizing: border-box !important;
		flex: 1 !important;
		min-height: 0 !important;
		overflow-y: auto !important;
	}

	/* Clear padding from the scroll container so its scrollbar is pinned to the far right edge */
	:global(.vditor-ir) {
		padding: 0 !important;
	}

	/* Hide the middle scrollbar in Split View (left pane) */
	:global(.vditor-sv)::-webkit-scrollbar {
		display: none !important;
		width: 0 !important;
		background: transparent !important;
	}
	:global(.vditor-sv) {
		scrollbar-width: none !important;
		-ms-overflow-style: none !important;
	}

	/* Keep the editor content readable on wide windows while allowing it to
	   shrink below that cap in a narrow pane so prose can wrap. */
	:global(.vditor-reset) {
		width: 100% !important;
		min-width: 0 !important;
		max-width: calc(120ch + 2 * var(--space-8)) !important;
		margin: 0 auto !important;
		padding-left: var(--space-8) !important;
		padding-right: var(--space-8) !important;
		white-space: pre-wrap !important;
		overflow-wrap: anywhere !important;
		word-break: break-word !important;
		overflow-x: hidden !important;
		box-sizing: border-box !important;
	}

	:global(.vditor-toolbar) {
		min-width: 0 !important;
		flex-wrap: wrap !important;
	}

	:global(.vditor-reset *) {
		max-width: 100%;
		overflow-wrap: anywhere;
		word-break: break-word;
	}

	/* Keep Markdown editing and preview readable in narrow panes. */
	:global(.vditor-reset),
	:global(.vditor-textarea) {
		font-size: 12px !important;
	}

	:global(.vditor-ir) {
		min-width: 0 !important;
		overflow-x: hidden !important;
	}

	:global(.vditor-preview__action) {
		display: none !important;
	}

	@media (min-width: 1200px) {
		:global(.vditor-content:has(.vditor-sv[style*='block'])) {
			flex-direction: row !important;
			align-items: stretch !important;
			justify-content: center !important;
			gap: 0 !important;
			padding: 0 !important;
		}

		:global(.vditor-content:has(.vditor-sv[style*='block']) .vditor-ir),
		:global(.vditor-content:has(.vditor-sv[style*='block']) .vditor-sv),
		:global(.vditor-content:has(.vditor-sv[style*='block']) .vditor-preview) {
			margin: 0 !important;
		}

		:global(.vditor-content:has(.vditor-sv[style*='block']) .vditor-reset) {
			padding-left: var(--space-6) !important;
			padding-right: var(--space-6) !important;
		}
	}

	:global(.vditor-reset),
	:global(.vditor-textarea) {
		font-family: var(--font-mono) !important;
	}

	:global(.vditor-ir),
	:global(.vditor-reset) {
		color: var(--text-primary) !important;
	}

	:global(.vditor-toolbar) {
		border-bottom: 1px solid var(--border-subtle) !important;
		padding: var(--space-2) var(--space-4) !important;
		padding-right: 120px !important;
		transition: max-height 0.2s ease-out;
		position: relative !important;
		z-index: 30 !important;
	}

	:global(.vditor-wrapper.has-pdf-notePage.note .vditor-toolbar) {
		padding-right: 120px !important;
	}

	/* Sidebar (Mobile / Overlay mode by default) */
	.sidebar {
		position: absolute;
		top: 0;
		right: 0;
		bottom: 0;
		width: var(--sidebar-width, 20rem);
		box-sizing: border-box;
		min-width: 0;
		max-width: 85vw;
		background: var(--bg-panel);
		padding: 0 var(--space-6) var(--space-6) var(--space-6);
		display: flex;
		flex-direction: column;
		gap: var(--space-6);
		overflow-y: auto;
		z-index: 100;
		transform: translateX(100%);
		transition:
			transform 0.3s cubic-bezier(0.16, 1, 0.3, 1),
			margin-right 0.3s cubic-bezier(0.16, 1, 0.3, 1);
		border-left: 1px solid var(--border-default);
		border-radius: 0 !important;
		box-shadow: -4px 0 24px var(--shadow-color);
		font-family: var(--font-mono);
	}

	.sidebar.open {
		transform: translateX(0);
	}

	.sidebar-backdrop {
		position: absolute;
		inset: 0;
		background: var(--scrim-soft);
		backdrop-filter: blur(var(--blur-sm));
		z-index: 90;
		animation: fade-in var(--duration-fast) ease-out;
	}

	/* Large Screen — sidebar docks side by side with the editor */
	@media (min-width: 1380px) {
		.sidebar {
			position: relative;
			transform: none;
			margin-right: calc(var(--sidebar-width, 20rem) * -1);
			/* Docking is allowed only when the editor can retain its pane minimum. */
			max-width: min(calc(100% - 20rem), 42rem);
			box-shadow: none;
			flex-shrink: 0;
		}

		.sidebar.open {
			transform: none;
			margin-right: 0;
		}

		.sidebar-backdrop {
			display: none !important;
		}
	}

	.sidebar-resizer {
		position: absolute;
		left: -3px;
		top: 0;
		bottom: 0;
		width: 6px;
		cursor: ew-resize;
		z-index: 1000;
		transition: background 0.2s ease;
	}
	.sidebar-resizer:hover,
	.sidebar-resizer.resizing {
		background: var(--accent-100);
	}

	.sidebar-section {
		display: flex;
		flex-direction: column;
		gap: var(--space-3);
	}

	.latex-preview-section {
		border-top: 1px solid var(--border-default);
		padding-top: var(--space-4);
	}

	.latex-preview-controls {
		display: flex;
		gap: var(--space-2);
	}

	.latex-preview-controls button {
		font-size: 0.75rem;
		white-space: nowrap;
	}

	.latex-preview-state {
		display: flex;
		align-items: flex-start;
		gap: var(--space-2);
		font-size: 0.75rem;
		color: var(--text-secondary);
		line-height: 1.4;
	}

	.latex-preview-state.error,
	.latex-error,
	.latex-diagnostics {
		color: var(--danger-400, #f87171);
	}

	.latex-status-dot {
		width: 0.45rem;
		height: 0.45rem;
		margin-top: 0.3rem;
		border-radius: 50%;
		background: var(--neutral-500);
		flex: 0 0 auto;
	}

	.latex-status-dot.active {
		background: var(--success-400, #4ade80);
	}
	.latex-status-dot.pending {
		background: var(--accent-200);
	}

	.latex-error,
	.latex-diagnostics {
		margin: 0;
		font-size: 0.72rem;
		line-height: 1.45;
		word-break: break-word;
	}

	.latex-diagnostics {
		padding-left: 1.1rem;
	}

	.latex-diagnostics span {
		font-family: var(--font-mono);
		font-size: 0.68rem;
	}

	.sidebar h3 {
		margin: 0;
		font-size: 0.75rem;
		text-transform: uppercase;
		letter-spacing: 0.08em;
		color: var(--text-primary);
	}

	.row-badge {
		border: 1px solid var(--border-default);
		border-radius: 4px;
		padding: 2px 6px;
		font-size: 0.65rem;
		color: var(--neutral-400);
		font-family: var(--font-mono);
		background: var(--overlay-faint);
		flex-shrink: 0;
		white-space: nowrap;
	}

	.tag-input {
		width: 100%;
		border: 1px solid var(--border-default);
		border-radius: var(--radius-xs);
		background: var(--bg-page);
		padding: 0.625rem 0.75rem;
		color: var(--text-primary);
		outline: none;
	}
	.tag-input:focus {
		border-color: var(--accent-200);
	}

	.empty-state {
		margin: 0;
		font-size: 0.875rem;
		color: var(--neutral-500);
		font-style: italic;
	}

	.related-list {
		list-style: none;
		padding: 0;
		margin: 0;
		display: flex;
		flex-direction: column;
		gap: var(--space-2);
	}

	.related-list a {
		color: var(--text-primary);
		text-decoration: none;
		font-size: 0.875rem;
		display: block;
		padding: 0.375rem 0;
		border-bottom: 1px solid transparent;
		transition: color var(--duration-fast);
	}
	.related-list a:hover {
		color: var(--accent-100);
	}

	.context-excerpt :global(.backlink-link-label) {
		color: var(--accent-200);
		font-weight: 500;
	}

	.context-excerpt :global(.backlink-block-label) {
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

	@media (max-width: 1024px) {
		.editor-header {
			flex-wrap: wrap;
			gap: var(--space-4);
			position: sticky;
			top: 0;
			z-index: 10;
		}
		.title-input {
			max-width: 100%;
		}
	}

	.math-dialog {
		padding: 0;
		border: 1px solid var(--border-default);
		border-radius: var(--radius-sm);
		background: var(--bg-panel);
		color: var(--text-primary);
		max-width: 40rem;
		width: 100%;
		backdrop-filter: blur(var(--blur-md));
		box-shadow: none;
	}
	.math-dialog::backdrop {
		background: var(--scrim);
		backdrop-filter: blur(var(--blur-sm));
	}
	.dialog-content {
		padding: var(--space-6);
		display: grid;
		gap: var(--space-4);
	}
	.dialog-content h3 {
		margin: 0;
		font-size: 1.25rem;
		color: var(--text-hero);
	}
	.math-container {
		min-height: 4rem;
	}
	.dialog-actions {
		display: flex;
		justify-content: flex-end;
		gap: var(--space-2);
		margin-top: var(--space-4);
	}
	.dialog-actions button {
		padding: 0.625rem 1rem;
		border-radius: var(--radius-sm);
		border: 1px solid var(--border-default);
		background: var(--bg-panel);
		color: var(--text-primary);
		cursor: pointer;
	}
	.dialog-actions .primary {
		background: var(--accent-200);
		color: var(--text-inverse);
		border-color: var(--accent-200);
	}

	.link-dialog {
		padding: 0;
		border: 1px solid var(--border-default);
		border-radius: var(--radius-sm);
		background: var(--bg-panel);
		color: var(--text-primary);
		max-width: 40rem;
		width: 100%;
		backdrop-filter: blur(var(--blur-md));
		outline: none;
		box-shadow: none;
	}
	.link-dialog::backdrop {
		background: var(--scrim);
		backdrop-filter: blur(var(--blur-sm));
	}
	.link-search-input {
		width: 100%;
		border: 2px solid var(--border-default);
		border-radius: var(--radius-sm);
		background: var(--bg-panel);
		padding: 1rem 1.25rem;
		font-size: 1.125rem;
		color: var(--text-primary);
		outline: none;
		font-family: var(--font-sans);
		margin-bottom: var(--space-4);
		transition: border-color 0.2s;
	}
	.link-search-input:focus {
		border-color: var(--accent-200);
	}

	.link-results-container {
		max-height: 300px;
		overflow-y: auto;
		border: 1px solid var(--border-subtle);
		border-radius: var(--radius-xs);
		background: var(--bg-panel);
		padding: var(--space-2);
	}

	.link-results-list {
		list-style: none;
		margin: 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: 2px;
	}

	.link-result-btn {
		width: 100%;
		display: flex;
		justify-content: space-between;
		align-items: center;
		padding: 0.5rem 0.75rem;
		background: transparent;
		border: none;
		border-radius: var(--radius-xs);
		color: var(--text-primary);
		cursor: pointer;
		text-align: left;
		transition: background 0.1s;
	}
	.link-result-btn:hover,
	.link-result-btn.selected {
		background: var(--accent-tint);
	}

	.folder-badge {
		font-size: 0.7rem;
		color: var(--text-secondary);
		background: var(--bg-page);
		padding: 0.125rem 0.375rem;
		border-radius: 1rem;
		border: 1px solid var(--border-subtle);
	}

	:global(.has-attached-file [data-type='attach-pdf']) {
		opacity: 0.3 !important;
		pointer-events: none !important;
		cursor: not-allowed !important;
	}

	.pdf-attach-dialog {
		width: 100%;
		max-width: 800px;
		background: var(--bg-modal);
		border: 1px solid var(--border-subtle);
		border-radius: var(--radius-md);
		color: var(--text-primary);
		box-shadow: 0 10px 30px var(--shadow-color-strong);
	}
	.pdf-attach-dialog::backdrop {
		background: var(--scrim);
		backdrop-filter: blur(2px);
	}
	.pdf-attach-dialog .dialog-content {
		padding: var(--space-6);
	}
	.pdf-attach-dialog h3 {
		margin-top: 0;
		font-size: 1.25rem;
		font-weight: 500;
	}
	.dialog-subtitle {
		font-size: 0.875rem;
		color: var(--text-secondary);
		margin-bottom: var(--space-4);
	}

	.pdf-grid-container {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(180px, 1fr));
		gap: 16px;
		margin-top: 12px;
		margin-bottom: 24px;
		max-height: 400px;
		overflow-y: auto;
		padding: 4px;
		padding-right: 8px;
	}

	.pdf-grid-container::-webkit-scrollbar {
		width: 6px;
	}
	.pdf-grid-container::-webkit-scrollbar-thumb {
		background: var(--border-default);
		border-radius: 4px;
	}

	.pdf-grid-upload-card {
		background: var(--overlay-faint);
		border: 1px dashed var(--border-default);
		border-radius: var(--radius-sm);
		padding: 24px 16px;
		display: flex;
		flex-direction: column;
		align-items: center;
		justify-content: center;
		gap: 8px;
		cursor: pointer;
		transition: all 0.2s ease;
		color: var(--text-primary);
		text-align: center;
		font-family: inherit;
	}
	.pdf-grid-upload-card:hover:not(:disabled) {
		background: var(--hover-overlay);
		border-color: var(--accent-300);
	}
	.pdf-grid-upload-card:disabled {
		opacity: 0.5;
		cursor: not-allowed;
	}
	.upload-icon-wrapper {
		color: var(--accent-300);
		margin-bottom: 4px;
	}
	.upload-text {
		font-weight: 500;
		font-size: 0.95rem;
	}
	.upload-subtext {
		font-size: 0.75rem;
		color: var(--text-secondary);
	}

	.pdf-grid-card {
		background: var(--overlay-faint);
		border: 1px solid var(--border-subtle);
		border-radius: var(--radius-sm);
		padding: 16px;
		display: flex;
		flex-direction: column;
		align-items: flex-start;
		gap: 12px;
		cursor: pointer;
		transition: all 0.2s ease;
		text-align: left;
		font-family: inherit;
		color: inherit;
	}
	.pdf-grid-card:hover {
		background: var(--hover-overlay-strong);
		border-color: var(--border-default);
		transform: translateY(-2px);
	}
	.pdf-card-icon {
		background: var(--bg-code);
		padding: 12px;
		border-radius: 8px;
		display: flex;
		align-items: center;
		justify-content: center;
	}
	.pdf-card-info {
		display: flex;
		flex-direction: column;
		gap: 4px;
		width: 100%;
	}
	.pdf-card-info strong {
		font-size: 0.9rem;
		font-weight: 500;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
		width: 100%;
	}
	.pdf-card-info span {
		font-size: 0.75rem;
		color: var(--text-secondary);
	}

	.preview-dialog {
		padding: 0;
		border: none;
		border-radius: var(--radius-md);
		background: transparent;
		color: var(--text-primary);
		width: 800px;
		max-width: 90vw;
		height: 75vh;
		max-height: 80vh;
		outline: none;
	}
	.preview-dialog::backdrop {
		background: var(--scrim-soft);
		backdrop-filter: blur(var(--blur-sm));
	}
	.preview-layout {
		display: flex;
		height: 100%;
		gap: var(--space-4);
		position: relative;
	}
	.preview-main {
		flex: 1;
		background: var(--bg-page);
		border-radius: var(--radius-md);
		border: 1px solid var(--border-default);
		display: flex;
		flex-direction: column;
		overflow: hidden;
		box-shadow: 0 12px 48px var(--shadow-color-strong);
	}
	.preview-header {
		padding: var(--space-6) var(--space-8);
		border-bottom: 1px solid var(--border-subtle);
		background: var(--bg-panel);
	}
	.preview-header h2 {
		margin: 0 0 var(--space-2) 0;
		font-size: 1.5rem;
		color: var(--text-hero);
	}
	.preview-meta span {
		font-family: var(--font-mono);
		font-size: 0.875rem;
		color: var(--text-secondary);
		background: var(--neutral-600);
		padding: 0.1rem 0.4rem;
		border-radius: var(--radius-xs);
	}
	.preview-content-scroll {
		flex: 1;
		overflow-y: auto;
		background: var(--bg-page);
	}
	.preview-sidebar {
		display: flex;
		flex-direction: column;
		gap: var(--space-3);
		padding-top: var(--space-4);
		align-items: center;
	}
	.icon-btn {
		width: 48px;
		height: 48px;
		border-radius: 50%;
		background: var(--bg-panel);
		border: 1px solid var(--border-default);
		color: var(--text-primary);
		display: flex;
		align-items: center;
		justify-content: center;
		cursor: pointer;
		transition: all 0.2s;
	}
	.icon-btn:hover {
		background: var(--neutral-600);
		color: var(--text-inverse);
	}

	/* Base State (Collapsed): Hide the link text completely to make the transclusion look seamless */
	:global(
		.transclusion-wrapper[data-block-content]:not([data-block-content='']):not(
				.vditor-ir__node--expand
			):not(.force-expand)
			.vditor-ir__link
	) {
		display: none !important;
	}
	/* Strip the wrapper pill background when collapsed because we only want the ::after to show */
	:global(
		.transclusion-wrapper[data-block-content]:not([data-block-content='']):not(
				.vditor-ir__node--expand
			):not(.force-expand)
	) {
		padding: 0 !important;
		background: transparent !important;
		border: none !important;
		display: block !important;
	}
	/* Ensure the ::after preview has no top margin since there's no text above it */
	:global(
		.transclusion-wrapper[data-block-content]:not([data-block-content='']):not(
				.vditor-ir__node--expand
			):not(.force-expand)::after
	) {
		margin-top: 0 !important;
	}

	/* Active State (Selected or Edited): Restore the orange pill styling */
	:global(.transclusion-wrapper.force-expand),
	:global(.transclusion-wrapper.vditor-ir__node--expand) {
		padding: 0.25rem 0.5rem !important;
		background: var(--accent-tint) !important;
		border-left: 3px solid var(--accent-200) !important;
		border-radius: 0 var(--radius-sm) var(--radius-sm) 0 !important;
		display: inline-block !important;
	}

	/* Style the link text nicely when active */
	:global(.transclusion-wrapper .vditor-ir__link) {
		color: var(--accent-200) !important;
		font-family: var(--font-mono) !important;
		font-size: 0.875em !important;
	}

	/* Render the block content seamlessly via pseudo-element */
	:global(.transclusion-wrapper::after) {
		content: attr(data-block-content);
		display: block;
		margin-top: 0.5rem;
		padding: var(--space-3) 1rem;
		background: var(--accent-tint);
		border-left: 3px solid var(--accent-200);
		border-radius: 0 var(--radius-sm) var(--radius-sm) 0;
		color: var(--text-secondary);
		font-family: var(--font-mono);
		white-space: pre-wrap;
		font-size: 0.85em;
		line-height: 1.5;
		cursor: default;
	}

	/* Hide the transclusion content when the link is actively selected or edited to prevent visual clutter */
	:global(.transclusion-wrapper.force-expand::after),
	:global(.transclusion-wrapper.vditor-ir__node--expand::after) {
		display: none !important;
	}

	/* Prevent Vditor from "truncating" (hiding) link markers ONLY when actively selected or edited */
	:global(.vditor-ir__node[data-type='a'].force-expand .vditor-ir__marker),
	:global(.vditor-ir__node[data-type='a'].vditor-ir__node--expand .vditor-ir__marker) {
		display: inline !important;
		opacity: 0.6;
		font-family: var(--font-mono);
	}

	/* Vditor link theme override */
	:global(.vditor-reset a),
	:global(.vditor-ir__link) {
		color: var(--accent-200) !important;
		text-decoration-color: var(--accent-200) !important;
	}

	/* Ensure Vditor fullscreen covers the custom titlebar */
	:global(.vditor--fullscreen) {
		z-index: 10000 !important;
	}

	/* Fullscreen Indicator */
	.fullscreen-indicator {
		display: none;
		position: fixed;
		bottom: var(--space-8);
		right: var(--space-8);
		background: var(--bg-panel-blur);
		color: var(--text-secondary);
		padding: var(--space-2) var(--space-4);
		border-radius: var(--radius-full);
		font-size: 0.875rem;
		pointer-events: none;
		z-index: 10001; /* Must be above Vditor's 10000 */
		backdrop-filter: blur(var(--blur-md));
		border: 1px solid var(--border-default);
		box-shadow: var(--shadow-lg);
	}

	.fullscreen-indicator span {
		background: var(--bg-panel);
		color: var(--text-primary);
		padding: 2px 6px;
		border-radius: var(--radius-xs);
		border: 1px solid var(--border-subtle);
		font-family: var(--font-mono);
		font-size: 0.75rem;
	}

	:global(.content-area:has(.vditor--fullscreen)) .fullscreen-indicator {
		display: flex;
		align-items: center;
		gap: var(--space-2);
		animation: fade-in 0.3s ease-out;
	}

	:global(.vditor-hint button[data-mode='wysiwyg']) {
		display: none !important;
	}

	/* Sidebar Tabs */
	.sidebar-tabs {
		display: flex;
		min-width: 0;
		width: 100%;
		box-sizing: border-box;
		height: 48px;
		border-bottom: 1px solid var(--border-subtle);
		margin-bottom: var(--space-4);
		flex-shrink: 0;
	}
	.sidebar-tabs button {
		flex: 1;
		background: transparent;
		border: none;
		color: var(--text-secondary);
		padding: 0;
		font-family: var(--font-sans);
		font-size: 0.875rem;
		cursor: pointer;
		border-bottom: 2px solid transparent;
		transition: all var(--duration-fast);
		display: flex;
		align-items: center;
		justify-content: center;
	}
	.sidebar-tabs button.active {
		color: var(--accent-100);
		border-bottom-color: var(--accent-100);
	}
	.sidebar-tabs button:hover:not(.active) {
		color: var(--text-primary);
	}

	.sidebar-content {
		display: flex;
		flex-direction: column;
		min-width: 0;
		width: 100%;
		box-sizing: border-box;
		gap: var(--space-6);
		flex: 1;
		min-height: 0;
		overflow-y: auto;
	}

	.sidebar-content::-webkit-scrollbar,
	.chat-messages::-webkit-scrollbar,
	.versions-container::-webkit-scrollbar,
	.sidebar::-webkit-scrollbar {
		display: none;
	}
	.sidebar-content,
	.chat-messages,
	.versions-container,
	.sidebar {
		-ms-overflow-style: none;
		scrollbar-width: none;
	}

	/* Chat UI */
	.chat-container {
		display: flex;
		flex-direction: column;
		height: 100%;
		flex: 1;
		min-width: 0;
	}
	.chat-messages {
		flex: 1;
		min-width: 0;
		overflow-y: auto;
		display: flex;
		flex-direction: column;
		gap: var(--space-4);
		padding-bottom: var(--space-4);
	}
	.chat-message {
		display: flex;
		flex-direction: column;
	}
	.chat-message.tool-only {
		margin-top: calc(-1 * var(--space-3));
		margin-bottom: calc(-1 * var(--space-3));
	}
	.chat-message.user {
		align-items: flex-end;
	}
	.chat-message.assistant {
		align-items: flex-start;
	}
	.chat-bubble {
		max-width: 85%;
		padding: var(--space-3) var(--space-4);
		border-radius: var(--radius-md);
		font-size: 0.875rem;
		line-height: 1.5;
		min-width: 0;
		word-break: break-word;
		overflow-wrap: anywhere;
	}
	.chat-message.user .chat-bubble {
		background: var(--accent-200);
		color: var(--on-accent);
	}
	.chat-message.assistant .chat-bubble {
		background: var(--bg-panel);
		color: var(--text-primary);
	}
	/* Markdown tables rendered inside assistant messages need an explicit
	   surface and text palette; otherwise WebKit falls back to a white table
	   while the surrounding chat bubble remains themed. */
	:global(.chat-bubble .selectable-content) {
		min-width: 0;
		overflow-x: auto;
	}
	:global(.chat-bubble table) {
		width: 100%;
		min-width: 24rem;
		overflow-x: auto;
		margin: var(--space-3) 0;
		border: 1px solid var(--border-default);
		border-collapse: collapse;
		background: var(--bg-panel);
		color: var(--text-secondary);
		font-size: 0.8rem;
		line-height: 1.45;
	}
	:global(.chat-bubble thead) {
		background: var(--bg-code);
	}
	:global(.chat-bubble th),
	:global(.chat-bubble td) {
		padding: var(--space-2) var(--space-3);
		border: 1px solid var(--border-default);
		text-align: left;
		vertical-align: top;
		white-space: nowrap;
		word-break: normal;
		overflow-wrap: normal;
	}
	:global(.chat-bubble th) {
		color: var(--text-primary);
		font-weight: 600;
	}
	:global(.chat-bubble tbody tr:nth-child(even)) {
		background: var(--overlay-faint);
	}
	:global(.chat-bubble tbody tr:hover) {
		background: var(--hover-overlay-strong);
	}
	.chat-message.tool-only .chat-bubble {
		padding-top: var(--space-1);
		padding-bottom: var(--space-1);
		background: transparent;
	}
	.chat-bubble.error {
		background: color-mix(in srgb, var(--bg-panel) 82%, var(--accent-200));
	}
	.chat-bubble.error {
		border-left: 3px solid var(--danger);
		background-color: var(--danger-bg);
	}

	.approval-card {
		background: var(--bg-code);
		border: 1px solid var(--accent-200);
		border-radius: var(--radius-md);
		padding: var(--space-3);
		margin: var(--space-2) 0;
		display: flex;
		flex-direction: column;
		gap: var(--space-2);
		width: 100%;
		box-sizing: border-box;
	}
	.approval-card.rejected {
		border-color: var(--danger-border);
		background: var(--danger-bg);
	}
	.approval-card.rejected .title {
		color: var(--danger-text);
	}

	.approval-card .title {
		margin: 0;
		font-size: 0.85rem;
		color: var(--accent-300);
	}

	.approval-card pre {
		background: var(--bg-code);
		padding: var(--space-2);
		border-radius: var(--radius-sm);
		font-size: 0.75rem;
		font-family: var(--font-mono);
		white-space: pre-wrap;
		word-break: break-word;
		max-height: 120px;
		overflow-y: auto;
		margin: 0;
		color: var(--text-muted);
	}

	.approval-card pre::-webkit-scrollbar {
		width: 6px;
		height: 6px;
	}
	.approval-card pre::-webkit-scrollbar-track {
		background: transparent;
	}
	.approval-card pre::-webkit-scrollbar-thumb {
		background: var(--neutral-700, #444);
		border-radius: 3px;
	}
	.approval-card pre::-webkit-scrollbar-thumb:hover {
		background: var(--neutral-500, #666);
	}

	.approval-actions {
		display: flex;
		gap: var(--space-2);
		flex-wrap: wrap; /* Fix responsiveness for narrow sidebars */
	}

	.pending-approval-bar {
		display: flex;
		justify-content: space-between;
		align-items: center;
		padding: var(--space-3) var(--space-4);
		background: var(--surface-2);
		border-top: 1px solid var(--border-subtle);
		border-bottom: 1px solid var(--border-subtle);
		gap: var(--space-4);
	}
	.pending-info {
		display: flex;
		align-items: center;
		gap: var(--space-2);
		font-size: 0.85rem;
		color: var(--text-primary);
	}
	.pending-actions {
		display: flex;
		gap: var(--space-2);
	}
	.pending-actions button {
		padding: 0.4rem 0.8rem;
		border-radius: var(--radius-sm);
		font-size: 0.8rem;
		cursor: pointer;
		font-weight: 500;
	}
	.pending-actions .primary {
		background: var(--accent-500);
		color: white;
		border: 1px solid var(--accent-500);
	}
	.pending-actions .secondary {
		background: transparent;
		color: var(--text-primary);
		border: 1px solid var(--border-default);
	}
	.chat-tools {
		display: flex;
		flex-direction: column;
		gap: var(--space-2);
		margin-bottom: var(--space-2);
	}
	.chat-tools.streaming {
		position: sticky;
		top: 0;
		z-index: 1;
		background: inherit;
		padding-bottom: var(--space-2);
	}
	.chat-input-area {
		display: flex;
		flex-direction: column;
		gap: var(--space-2);
		padding-top: var(--space-3);
		border-top: 1px solid var(--border-subtle);
	}
	.prompt-box {
		background: var(--neutral-1000);
		border: 1px solid var(--border-default);
		border-radius: var(--radius-md);
		padding: var(--space-2);
		display: flex;
		flex-direction: column;
		gap: 2px;
		transition:
			border-color 0.15s ease,
			box-shadow 0.15s ease;
	}
	.prompt-box:focus-within {
		border-color: color-mix(in srgb, var(--accent-200) 55%, var(--border-default));
		box-shadow: 0 0 0 3px color-mix(in srgb, var(--accent-200) 12%, transparent);
	}
	.chat-input-area textarea {
		width: 100%;
		background: transparent;
		border: none;
		border-radius: 0;
		padding: var(--space-1) var(--space-2);
		color: var(--text-primary);
		outline: none;
		resize: none;
		font-family: inherit;
		font-size: 0.875rem;
		line-height: 1.45;
		overflow-y: auto;
	}
	.chat-input-area textarea::placeholder {
		color: var(--neutral-500);
	}
	.write-target-notice {
		margin: 2px var(--space-2) 4px;
		padding: 7px 9px;
		border: 1px solid color-mix(in srgb, var(--accent-300) 35%, var(--border-default));
		border-radius: var(--radius-sm);
		background: color-mix(in srgb, var(--accent-500) 8%, transparent);
		color: var(--text-secondary);
		font-size: 0.72rem;
		line-height: 1.4;
	}
	.prompt-toolbar {
		display: flex;
		align-items: center;
		gap: 5px;
		padding: 2px;
	}
	.prompt-spacer {
		flex: 1;
	}

	.interaction-mode {
		display: inline-flex;
		height: 28px;
		padding: 2px;
		border: 1px solid var(--border-default);
		border-radius: var(--radius-md);
		background: var(--neutral-950);
	}
	.interaction-mode button {
		padding: 0 9px;
		border: 0;
		border-radius: calc(var(--radius-md) - 2px);
		background: transparent;
		color: var(--text-secondary);
		font-size: 0.68rem;
		font-weight: 600;
		cursor: pointer;
	}
	.interaction-mode button:hover {
		color: var(--text-primary);
	}
	.interaction-mode button.active {
		background: var(--accent-100);
		color: var(--bg-base);
	}

	/* Shared control baseline: same height, calm by default, theme-consistent. */
	.prompt-icon-btn,
	.send-btn,
	.selection-pill {
		height: 28px;
		display: inline-flex;
		align-items: center;
		justify-content: center;
		border: none;
		background: transparent;
		color: var(--text-secondary);
		cursor: pointer;
		border-radius: var(--radius-md);
		white-space: nowrap;
		transition:
			background 0.14s ease,
			color 0.14s ease,
			border-color 0.14s ease,
			box-shadow 0.14s ease,
			opacity 0.14s ease;
	}

	.prompt-icon-btn {
		width: 28px;
	}
	.prompt-icon-btn:hover {
		background: var(--neutral-900);
		color: var(--text-primary);
	}
	.selection-pill {
		gap: 6px;
		padding: 0 5px 0 9px;
		border-radius: var(--radius-md);
		font-size: 0.72rem;
		font-weight: 600;
		color: var(--accent-100);
		background: color-mix(in srgb, var(--accent-100) 9%, transparent);
	}
	.selection-pill:hover {
		background: color-mix(in srgb, var(--accent-100) 15%, transparent);
	}
	.selection-pill svg {
		opacity: 0.75;
	}
	.selection-pill .sel-x {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		width: 16px;
		height: 16px;
		border-radius: 50%;
		opacity: 0.5;
		font-size: 0.66rem;
	}
	.selection-pill:hover .sel-x {
		opacity: 0.95;
		background: color-mix(in srgb, var(--accent-100) 22%, transparent);
	}

	.send-btn {
		width: 28px;
	}
	.send-btn:not(:disabled) {
		background: var(--accent-200);
		color: var(--on-accent);
	}
	.send-btn:hover:not(:disabled) {
		background: var(--accent-100);
	}
	.send-btn:disabled {
		background: transparent;
		color: var(--neutral-600);
		cursor: default;
	}

	.chat-input-area textarea::-webkit-scrollbar {
		width: 6px;
	}
	.chat-input-area textarea::-webkit-scrollbar-track {
		background: transparent;
	}
	.chat-input-area textarea::-webkit-scrollbar-thumb {
		background: var(--neutral-700, #444);
		border-radius: 3px;
	}
	.chat-input-area textarea::-webkit-scrollbar-thumb:hover {
		background: var(--neutral-500, #666);
	}

	.debug-window {
		background: var(--neutral-900, #1a1a2e);
		border: 1px solid var(--border-subtle, #333);
		border-radius: var(--radius-md);
		font-size: 0.7rem;
		font-family: var(--font-mono, monospace);
		color: var(--text-secondary, #aaa);
		overflow: hidden;
	}
	.debug-window-header {
		display: flex;
		justify-content: space-between;
		align-items: center;
		padding: 4px 8px;
		border-bottom: 1px solid var(--border-subtle, #333);
		background: var(--neutral-950, #0d0d1a);
	}
	.debug-title {
		font-weight: 600;
		font-size: 0.68rem;
		text-transform: uppercase;
		letter-spacing: 0.5px;
		color: var(--text-tertiary, #777);
	}
	.debug-toggle {
		background: none;
		border: none;
		color: var(--text-tertiary, #777);
		cursor: pointer;
		padding: 0 2px;
		font-size: 0.85rem;
		line-height: 1;
		border-radius: 3px;
	}
	.debug-toggle:hover {
		color: var(--text-primary);
		background: var(--neutral-800, #222);
	}
	.debug-grid {
		display: grid;
		grid-template-columns: 1fr 1fr;
		gap: 1px;
		padding: 4px 8px;
	}
	.debug-row {
		display: flex;
		justify-content: space-between;
		padding: 1px 0;
	}
	.debug-label {
		color: var(--text-tertiary, #777);
	}
	.debug-value {
		color: var(--text-primary, #ddd);
		font-weight: 500;
		text-align: right;
	}
	.debug-note {
		grid-column: 1 / -1;
		color: var(--text-tertiary, #777);
		font-size: 0.65rem;
		padding: 2px 8px 4px;
		font-style: italic;
	}
	.debug-trace {
		max-height: 120px;
		overflow-y: auto;
		border-top: 1px solid var(--border-subtle, #333);
		padding: 4px 8px;
		display: flex;
		flex-direction: column;
		gap: 2px;
		font-size: 0.65rem;
	}
	.trace-entry {
		display: flex;
		gap: 6px;
		align-items: baseline;
		padding: 1px 0;
	}
	.trace-time {
		color: var(--text-tertiary, #555);
		flex-shrink: 0;
		min-width: 38px;
		font-variant-numeric: tabular-nums;
	}
	.trace-msg {
		color: var(--text-secondary, #aaa);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}
	.trace-prompt {
		flex: 1;
		min-width: 0;
		color: var(--accent-100, #93c5fd);
	}
	.trace-prompt summary {
		cursor: pointer;
		user-select: none;
	}
	.trace-prompt pre {
		margin: 0.4rem 0 0;
		padding: 0.45rem;
		max-height: 18rem;
		overflow: auto;
		white-space: pre-wrap;
		word-break: break-word;
		font: inherit;
		color: var(--text-secondary, #aaa);
		background: var(--bg-raised, rgba(0, 0, 0, 0.2));
		border-radius: 3px;
	}
	.trace-entry.tool .trace-msg {
		color: var(--accent-200, #6ea8fe);
	}
	.trace-entry.gen .trace-msg {
		color: var(--text-primary, #ddd);
	}
	.trace-entry.note .trace-msg {
		color: var(--accent-100, #93c5fd);
	}
	.trace-entry.done .trace-msg {
		color: #4ade80;
	}
	.trace-entry.error .trace-msg {
		color: var(--danger, #ef4444);
	}
	.trace-entry.send .trace-msg {
		color: var(--text-tertiary, #777);
	}
	.trace-entry.config .trace-msg {
		color: #c084fc;
	}
	.trace-entry.usage .trace-msg {
		color: #38bdf8;
	}
	.trace-entry.tool_result .trace-msg {
		color: #4ade80;
	}

	.loading-dots .loading-dots::after {
		content: '...';
		animation: blink 1.5s steps(4, end) infinite;
	}
	@keyframes blink {
		0%,
		20% {
			color: transparent;
		}
		40% {
			color: inherit;
		}
		100% {
			color: inherit;
		}
	}

	.chat-msg-actions {
		display: flex;
		gap: var(--space-1);
		margin-top: 3px;
		justify-content: flex-end;
	}
	.chat-msg-actions.assistant {
		justify-content: flex-start;
	}
	.copy-btn {
		display: inline-flex;
		align-items: center;
		justify-content: center;
	}
	.rewind-btn {
		padding: 1px 6px;
		font-size: 13px;
		background: transparent;
		border: 1px solid transparent;
		border-radius: var(--radius-sm);
		color: color-mix(in srgb, var(--text-tertiary) 50%, transparent);
		cursor: pointer;
		transition:
			color 0.15s,
			border-color 0.15s;
		line-height: 1.6;
	}
	.rewind-btn:hover {
		color: var(--text-secondary);
		border-color: var(--border-color);
	}
	.rewind-btn.retry:hover {
		color: var(--accent);
	}

	/* Version History UI */
	.versions-container {
		display: flex;
		flex-direction: column;
		height: 100%;
	}
	.history-list {
		list-style: none;
		padding: 0;
		margin: 0;
		display: flex;
		flex-direction: column;
		gap: var(--space-4);
	}
	.history-list li {
		background: var(--bg-panel);
		border: 1px solid var(--border-default);
		border-radius: var(--radius-sm);
		padding: var(--space-3);
		display: flex;
		flex-direction: column;
		gap: var(--space-2);
	}
	.commit-header {
		display: flex;
		flex-direction: column;
	}
	.commit-header strong {
		font-size: 0.875rem;
		color: var(--text-primary);
	}
	.commit-date {
		font-size: 0.75rem;
		color: var(--text-secondary);
	}
	.commit-actions {
		display: flex;
		gap: var(--space-2);
		margin-top: var(--space-2);
	}
	.commit-actions button {
		flex: 1;
		font-size: 0.75rem;
		padding: var(--space-1) 0;
	}

	.chat-time-taken {
		display: block;
		font-size: 0.75rem;
		color: var(--text-secondary);
		opacity: 0.7;
		margin-top: var(--space-2);
		user-select: none;
	}

	.chat-time-taken.live {
		color: var(--accent);
		opacity: 0.9;
	}

	.chat-progress {
		display: block;
		color: var(--text-secondary);
		font-size: 0.82rem;
		line-height: 1.35;
		margin-bottom: var(--space-1);
	}

	/* Animated "working" dots shown while a turn is running but has produced no
	   text yet (model thinking, or a tool like web search/fetch executing) — so a
	   slow turn reads as alive, not frozen. */
	.chat-working {
		display: inline-flex;
		gap: 4px;
		align-items: center;
		margin-top: var(--space-2);
		margin-right: var(--space-2);
		vertical-align: middle;
	}

	.chat-working span {
		width: 5px;
		height: 5px;
		border-radius: 50%;
		background: var(--accent);
		animation: chat-working-pulse 1.2s ease-in-out infinite;
	}

	.chat-working span:nth-child(2) {
		animation-delay: 0.2s;
	}

	.chat-working span:nth-child(3) {
		animation-delay: 0.4s;
	}

	@keyframes chat-working-pulse {
		0%,
		80%,
		100% {
			opacity: 0.25;
			transform: scale(0.8);
		}
		40% {
			opacity: 1;
			transform: scale(1);
		}
	}
</style>
