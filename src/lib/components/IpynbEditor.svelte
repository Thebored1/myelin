<script lang="ts">
	import { onMount } from 'svelte';
	import { loadPyodide } from 'pyodide';

	interface Props {
		value: string;
		onInput: (val: string) => void;
	}
	let { value, onInput }: Props = $props();

	type Cell = {
		cell_type: 'markdown' | 'code';
		source: string[];
		outputs?: any[];
		execution_count?: number | null;
	};

	type Notebook = {
		cells: Cell[];
		metadata: any;
		nbformat: number;
		nbformat_minor: number;
	};

	let notebook: Notebook = $state({ cells: [], metadata: {}, nbformat: 4, nbformat_minor: 5 });
	let parseError = $state<string | null>(null);

	$effect(() => {
		try {
			if (value.trim()) {
				notebook = JSON.parse(value);
				parseError = null;
			} else {
				notebook = { cells: [], metadata: {}, nbformat: 4, nbformat_minor: 5 };
				parseError = null;
			}
		} catch (e) {
			parseError = 'Invalid notebook JSON';
		}
	});

	function updateCell(index: number, newSource: string) {
		if (parseError) return;
		notebook.cells[index].source = newSource.split('\n').map((line, i, arr) => line + (i < arr.length - 1 ? '\n' : ''));
		onInput(JSON.stringify(notebook, null, 2));
	}

	let pyodideInstance: any = null;
	let pyodideLoading = $state(false);

	async function getPyodide() {
		if (pyodideInstance) return pyodideInstance;
		if (pyodideLoading) return new Promise(resolve => setTimeout(async () => resolve(await getPyodide()), 500));
		
		pyodideLoading = true;
		try {
			pyodideInstance = await loadPyodide({
				indexURL: 'https://cdn.jsdelivr.net/pyodide/v0.25.0/full/'
			});
			return pyodideInstance;
		} finally {
			pyodideLoading = false;
		}
	}

	async function executeCell(index: number) {
		const cell = notebook.cells[index];
		if (cell.cell_type !== 'code') return;
		
		const canExecute = localStorage.getItem('myelin_jupyter_exec') === 'true';
		if (!canExecute) {
			alert('Jupyter code execution is disabled. Enable it in Settings to run code blocks.');
			return;
		}

		const code = cell.source.join('');
		try {
			const pyodide = await getPyodide();
			
			let stdoutLines: string[] = [];
			let stderrLines: string[] = [];
			
			pyodide.setStdout({ batched: (msg: string) => stdoutLines.push(msg + '\n') });
			pyodide.setStderr({ batched: (msg: string) => stderrLines.push(msg + '\n') });
			
			await pyodide.runPythonAsync(code);
			
			const outputs = [];
			if (stdoutLines.length > 0) {
				outputs.push({ output_type: 'stream', name: 'stdout', text: stdoutLines });
			}
			if (stderrLines.length > 0) {
				outputs.push({ output_type: 'stream', name: 'stderr', text: stderrLines });
			}
			
			cell.outputs = outputs;
			cell.execution_count = (cell.execution_count || 0) + 1;
			onInput(JSON.stringify(notebook, null, 2));
		} catch (e) {
			cell.outputs = [{
				output_type: 'stream',
				name: 'stderr',
				text: [String(e)]
			}];
			onInput(JSON.stringify(notebook, null, 2));
		}
	}

	function addCell(type: 'markdown' | 'code') {
		notebook.cells.push({
			cell_type: type,
			source: [],
			...(type === 'code' ? { outputs: [], execution_count: null } : {})
		});
		onInput(JSON.stringify(notebook, null, 2));
	}
</script>

<div class="ipynb-editor">
	{#if parseError}
		<div class="error">{parseError}</div>
	{:else}
		<div class="cells">
			{#each notebook.cells as cell, i}
				<div class="cell {cell.cell_type}">
					<div class="cell-header">
						<span>{cell.cell_type === 'code' ? `In [${cell.execution_count || ' '}]` : 'Markdown'}</span>
						{#if cell.cell_type === 'code'}
							<button class="run-btn" onclick={() => executeCell(i)}>▶ Run</button>
						{/if}
					</div>
					<textarea
						class="cell-input"
						value={cell.source.join('')}
						oninput={(e) => updateCell(i, (e.target as HTMLTextAreaElement).value)}
						rows={Math.max(2, cell.source.length)}
					></textarea>
					{#if cell.cell_type === 'code' && cell.outputs && cell.outputs.length > 0}
						<div class="cell-outputs">
							{#each cell.outputs as out}
								{#if out.text}
									<pre>{out.text.join('')}</pre>
								{/if}
							{/each}
						</div>
					{/if}
				</div>
			{/each}
		</div>
		<div class="add-buttons">
			<button class="secondary" onclick={() => addCell('code')}>+ Code</button>
			<button class="secondary" onclick={() => addCell('markdown')}>+ Markdown</button>
		</div>
	{/if}
</div>

<style>
	.ipynb-editor {
		padding: 1rem;
		height: 100%;
		overflow-y: auto;
		background: var(--bg-page);
	}
	.cell {
		margin-bottom: 1rem;
		border: 1px solid var(--border-default);
		border-radius: var(--radius-sm);
		background: var(--bg-panel);
		overflow: hidden;
	}
	.cell-header {
		padding: 0.25rem 0.5rem;
		background: var(--bg-body);
		font-family: var(--font-mono);
		font-size: 0.75rem;
		color: var(--text-secondary);
		display: flex;
		justify-content: space-between;
		align-items: center;
	}
	.run-btn {
		background: none;
		border: none;
		color: var(--accent-400);
		cursor: pointer;
		font-size: 0.75rem;
	}
	.run-btn:hover {
		color: var(--accent-300);
	}
	.cell-input {
		width: 100%;
		border: none;
		resize: vertical;
		padding: 0.5rem;
		background: transparent;
		color: var(--text-primary);
		font-family: var(--font-mono);
		font-size: 0.85rem;
		line-height: 1.4;
		outline: none;
	}
	.markdown .cell-input {
		font-family: var(--font-sans);
		font-size: 0.9rem;
	}
	.cell-outputs {
		padding: 0.5rem;
		background: var(--bg-body);
		border-top: 1px solid var(--border-default);
	}
	.cell-outputs pre {
		margin: 0;
		font-family: var(--font-mono);
		font-size: 0.8rem;
		color: var(--text-primary);
		white-space: pre-wrap;
	}
	.add-buttons {
		display: flex;
		gap: 0.5rem;
		justify-content: center;
		margin-top: 1rem;
	}
	.error {
		color: var(--danger);
		padding: 1rem;
	}
</style>
