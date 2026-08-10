<script lang="ts">
import NotebookSelect from '$lib/components/NotebookSelect.svelte';
import type { HomeController } from '$lib/home/controller.svelte';
let { home }: { home: HomeController } = $props();
</script>

					<!-- Right: note details (selected row) or tasks -->
					{#if home.selectedNote}
						<div class="dash-right">
							<section class="dash-panel">
								<div class="panel-head">
									<h3>details</h3>
									<button
										class="panel-close"
										onclick={() => (home.selectedNote = null)}
										aria-label="Close details"
										title="Close"
									>
										<svg
											width="14"
											height="14"
											viewBox="0 0 24 24"
											fill="none"
											stroke="currentColor"
											stroke-width="2"
											stroke-linecap="round"
											stroke-linejoin="round"
											><line x1="18" y1="6" x2="6" y2="18" /><line
												x1="6"
												y1="6"
												x2="18"
												y2="18"
											/></svg
										>
									</button>
								</div>
								<div class="detail-body">
									<div class="detail-head">
										<div class="detail-title">{home.selectedNote.title}</div>
										<span class="row-badge">{home.getNoteBadge(home.selectedNote)}</span>
									</div>
									{#if home.selectedNote.tags.length > 0}
										<div class="detail-tags">
											{#each home.selectedNote.tags as tag}<span class="kc-tag">#{tag}</span>{/each}
										</div>
									{/if}
									<div class="detail-rows">
										<div class="detail-row">
											<span class="dr-key">created</span><span class="dr-val"
												>{home.fullDateTime(home.selectedNote.createdAt)}</span
											>
										</div>
										<div class="detail-row">
											<span class="dr-key">modified</span><span class="dr-val"
												>{home.fullDateTime(home.selectedNote.updatedAt)}</span
											>
										</div>
										<div class="detail-row">
											<span class="dr-key">folder</span><span class="dr-val"
												>{home.folderLabel(home.selectedNote)}</span
											>
										</div>
										<div class="detail-row">
											<span class="dr-key">path</span><span
												class="dr-val"
												title={home.selectedNote.relativePath}>{home.selectedNote.relativePath}</span
											>
										</div>
										<div class="detail-row">
											<span class="dr-key">links</span><span class="dr-val"
												>{home.selectedNote.backlinks.length}</span
											>
										</div>
									</div>
									{#if home.selectedNote.excerpt}
										<p class="detail-excerpt">{home.selectedNote.excerpt}</p>
									{/if}
									<button
										class="btn-primary detail-open"
										onclick={() => home.selectedNote && home.openNote(home.selectedNote.id)}>Open</button
									>
								</div>
							</section>
						</div>
					{:else if !home.tasksCollapsed}
						<div class="dash-right">
							<section class="dash-panel">
								<div class="panel-head">
									<h3>tasks</h3>
									<div class="panel-tabs">
										<button
											class:active={home.activeTaskFilter === 'all'}
											onclick={() => (home.activeTaskFilter = 'all')}>all</button
										>
										<button
											class:active={home.activeTaskFilter === 'active'}
											onclick={() => (home.activeTaskFilter = 'active')}>active</button
										>
										<button
											class:active={home.activeTaskFilter === 'done'}
											onclick={() => (home.activeTaskFilter = 'done')}>done</button
										>
									</div>
								</div>
								<div class="task-list">
									{#each home.filteredTasks as task (task.id)}
										<div class="task-card" class:expanded={home.expandedTaskId === task.id}>
											<div class="task-item" class:done={task.done}>
												<button
													class="subtask-circle main-task-circle"
													class:done={task.done}
													onclick={() => (task.done = !task.done)}
													tabindex="-1"
												></button>
												<input
													class="task-text-input"
													type="text"
													bind:value={task.text}
													onfocus={() => (home.expandedTaskId = task.id)}
												/>
												<button
													class="task-remove"
													tabindex="-1"
													onclick={(e) => {
														e.preventDefault();
														home.removeTask(task.id);
													}}
													aria-label="Remove task">&times;</button
												>
											</div>
											{#if home.expandedTaskId === task.id}
												<div class="task-expanded-details redesigned">
													<div class="field-row notebook-row">
									<NotebookSelect bind:value={task.notebook} notebooks={home.notebooks} />
													</div>

													<div class="field-row">
														<svg
															class="field-icon"
															viewBox="0 0 24 24"
															stroke="currentColor"
															stroke-width="2"
															fill="none"><path d="M4 6h16M4 12h16M4 18h16" /></svg
														>
														<textarea
															rows="1"
															use:home.autoResize
															placeholder="Add details"
															bind:value={task.details}
															class="field-input textarea-new"
														></textarea>
													</div>

													<div class="field-row">
														<svg
															class="field-icon"
															viewBox="0 0 24 24"
															stroke="currentColor"
															stroke-width="2"
															fill="none"
															><circle cx="12" cy="12" r="10" /><circle
																cx="12"
																cy="12"
																r="6"
															/><circle cx="12" cy="12" r="2" /></svg
														>
														<input
															type="text"
															placeholder="Add deadline"
															bind:value={task.dueDate}
															class="field-input date-time-new"
															onfocus={(e) => (e.currentTarget.type = 'date')}
															onblur={(e) => {
																if (!e.currentTarget.value) e.currentTarget.type = 'text';
															}}
														/>
													</div>

													<div class="field-row">
														<svg
															class="field-icon"
															viewBox="0 0 24 24"
															stroke="currentColor"
															stroke-width="2"
															fill="none"
															><circle cx="12" cy="12" r="10" /><path d="M12 6v6l4 2" /></svg
														>
														<input
															type="text"
															placeholder="Add date/time"
															bind:value={task.dueTime}
															class="field-input date-time-new"
															onfocus={(e) => (e.currentTarget.type = 'time')}
															onblur={(e) => {
																if (!e.currentTarget.value) e.currentTarget.type = 'text';
															}}
														/>
													</div>

													<div class="subtasks-container">
														{#if task.subtasks}
															{#each task.subtasks as subtask, i}
																<div class="subtask-row">
																	<svg
																		class="subtask-arrow"
																		viewBox="0 0 24 24"
																		stroke="currentColor"
																		stroke-width="2"
																		fill="none"
																		><path d="M6 4v6a2 2 0 0 0 2 2h10" /><path
																			d="M15 9l3 3-3 3"
																		/></svg
																	>
																	<button
																		class="subtask-circle"
																		class:done={subtask.done}
																		onclick={() => (subtask.done = !subtask.done)}
																		tabindex="-1"
																	></button>
																	<input
																		type="text"
																		bind:value={subtask.text}
																		class="field-input subtask-input-new"
																		class:done={subtask.done}
																	/>
																	<button
																		class="subtask-remove"
																		tabindex="-1"
																		onclick={() => task.subtasks!.splice(i, 1)}>&times;</button
																	>
																</div>
															{/each}
														{/if}
														<div class="subtask-row">
															<svg
																class="subtask-arrow"
																viewBox="0 0 24 24"
																stroke="currentColor"
																stroke-width="2"
																fill="none"
																><path d="M6 4v6a2 2 0 0 0 2 2h10" /><path d="M15 9l3 3-3 3" /></svg
															>
															<div class="subtask-circle empty"></div>
															<input
																type="text"
																placeholder="Enter title"
																class="field-input subtask-input-new"
																onkeydown={(e) => {
																	if (e.key === 'Enter' && e.currentTarget.value.trim()) {
																		e.preventDefault();
																		task.subtasks = task.subtasks || [];
																		task.subtasks.push({
																			id: Date.now(),
																			text: e.currentTarget.value.trim(),
																			done: false
																		});
																		e.currentTarget.value = '';
																	}
																}}
															/>
														</div>
														<div class="subtask-add-hint">Add subtasks</div>
													</div>
												</div>
											{/if}
										</div>
									{/each}
									{#if home.filteredTasks.length === 0}
										<div class="nb-empty">
											No {home.activeTaskFilter === 'all' ? '' : home.activeTaskFilter + ' '}tasks.
										</div>
									{/if}
								</div>
								<form
									class="add-task-form"
									onsubmit={(e) => {
										e.preventDefault();
										home.addTask();
									}}
								>
									<input type="text" placeholder="Add a task..." bind:value={home.newTaskText} />
									<button
										type="submit"
										class="btn-primary"
										style="padding: 6px 14px; border-radius: var(--radius-xs); font-size: 0.8rem; font-weight: 500; min-height: unset; line-height: 1;"
										>Add</button
									>
								</form>
							</section>
						</div>
					{/if}
