<script lang="ts">
	import type { NotePageController } from '$lib/note-page/controller.svelte';
	import type { ChatMessage } from '$lib/types';
	import ChatToolIndicator from '$lib/components/ChatToolIndicator.svelte';
	import { hideThinkingContent } from '$lib/chatContent';
	let { notePage }: { notePage: NotePageController } = $props();
</script>

<div class="chat-container">
	{#if notePage.chatPersistenceError}
		<div class="chat-persistence-error" role="alert">{notePage.chatPersistenceError}</div>
	{/if}
	<div
		class="chat-messages"
		bind:this={notePage.chatMessagesEl}
		onscroll={notePage.handleChatScroll}
	>
		{#if notePage.chatMessages.length === 0}
			<p class="empty-state">Ask me anything about this note or your library!</p>
		{:else}
			{#each notePage.chatMessages as msg, i (msg.id ?? i)}
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
									{#each msg.tools as tool, toolIndex (tool.name ?? toolIndex)}
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
											(msg.approvalStatus === 'rejected' ? 'Rejected tool: ' : 'Pending tool: ') +
											msg.approvalTool,
										details: msg.approvalDetails || ''
									}}
								/>
							{:else if msg.role === 'assistant' && visibleContent}
								<div class="selectable-content">
									<!-- renderChatContent sanitizes marked output with DOMPurify. -->
									<!-- eslint-disable-next-line svelte/no-at-html-tags -->
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

	{#if notePage.chatMessages.find((m: ChatMessage) => m.isApprovalRequest && m.approvalStatus === 'pending')}
		{@const pendingReq = notePage.chatMessages.find(
			(m: ChatMessage) => m.isApprovalRequest && m.approvalStatus === 'pending'
		)}
		<div class="pending-approval-bar">
			<div class="pending-info">
				<span class="tool-icon">⚡</span>
				<span class="pending-text">AI wants to use <strong>{pendingReq?.approvalTool}</strong></span
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
								? (
										(notePage.debugInfo.firstChunk - notePage.debugInfo.requestStart) /
										1000
									).toFixed(2) + 's'
								: '—'}</span
						>
					</div>
					<div class="debug-row">
						<span class="debug-label">First token → Done:</span>
						<span class="debug-value"
							>{notePage.debugInfo.done && notePage.debugInfo.firstChunk
								? ((notePage.debugInfo.done - notePage.debugInfo.firstChunk) / 1000).toFixed(2) +
									's'
								: '—'}</span
						>
					</div>
					<div class="debug-row">
						<span class="debug-label">Total elapsed:</span>
						<span class="debug-value"
							>{notePage.debugInfo.done
								? (
										(notePage.debugInfo.done - (notePage.debugInfo.requestStart ?? 0)) /
										1000
									).toFixed(2) + 's'
								: notePage.debugInfo.requestStart
									? ((Date.now() - notePage.debugInfo.requestStart) / 1000).toFixed(2) + 's (live)'
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
					{#each notePage.debugInfo.trace as entry, traceIndex (entry.time ?? traceIndex)}
						<div class="trace-entry {entry.kind}">
							<span class="trace-time"
								>+{((entry.time - (notePage.debugInfo.requestStart ?? entry.time)) / 1000).toFixed(
									1
								)}s</span
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
					Place the cursor where you want to write, or select text to rewrite. Then send again.
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
						><line x1="12" y1="5" x2="12" y2="19" /><line x1="5" y1="12" x2="19" y2="12" /></svg
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
						<svg width="15" height="15" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true"
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
							><line x1="12" y1="19" x2="12" y2="5" /><polyline points="5 12 12 5 19 12" /></svg
						>
					</button>
				{/if}
			</div>
		</div>
	</div>
</div>

<style>
	.chat-persistence-error {
		margin: 0.35rem 0.5rem;
		padding: 0.45rem 0.6rem;
		border: 1px solid var(--danger-border);
		border-radius: 0.35rem;
		color: var(--text-error);
		font-size: 0.78rem;
	}
</style>
