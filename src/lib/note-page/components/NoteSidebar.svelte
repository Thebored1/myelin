<script lang="ts">
	import type { NotePageController } from '$lib/note-page/controller.svelte';
	import { noteSidebarOpen } from '$lib/stores';
	import ChatToolIndicator from '$lib/components/ChatToolIndicator.svelte';
	import { formatBacklinkContext } from '$lib/backlinkContext';
	import { hideThinkingContent } from '$lib/chatContent';

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

