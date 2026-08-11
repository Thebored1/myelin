<script lang="ts">
	import type { NotePageController } from '$lib/note-page/controller.svelte';
	import NoteSidebar from '$lib/note-page/components/NoteSidebar.svelte';

	let { notePage }: { notePage: NotePageController } = $props();
</script>

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
			{#if notePage.saveStatus === 'unsaved'}
				<button class="retry-save" onclick={() => void notePage.saveNote()} disabled={notePage.isBusy}>
					Retry save
				</button>
			{/if}
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
									stroke="var(--danger)"
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


		<NoteSidebar {notePage} />
	{/if}
	</div>
</div>
