/** Owns lazy MathLive/KaTeX loading and formula validation. */
export function createMathSession(ctx: Record<string, any>) {
	let mathDialog: HTMLDialogElement | undefined = $state();
	let mathValue = $state('');
	let mathLiveReady = $state(false);
	let katexRenderer = $state<any>(null);
	let mathError = $state('');
	function mathToKatex(raw: string): string {
		return raw.replace(/\\(?:_)?placeholder(?:\[.*?\])?(?:{})?/g, '\\square');
	}

	async function openMathDialog() {
		try {
			const [{ default: katex }, _mathlive] = await Promise.all([import('katex'), import('mathlive')]);
			katexRenderer = katex;
			mathLiveReady = true;
			mathValue = '';
			mathDialog?.showModal();
		} catch (error) {
			console.error('Failed to load math support', error);
			ctx.message = 'Could not load math support.';
		}
	}

	$effect(() => {
		const value = mathValue;
		if (!value.trim() || !katexRenderer) {
			mathError = '';
			return;
		}
		try {
			katexRenderer.renderToString(mathToKatex(value), { throwOnError: true, displayMode: true });
			mathError = '';
		} catch (error: any) {
			mathError = error?.message ? String(error.message) : 'KaTeX cannot render this formula.';
		}
	});

	function insertMath() {
		if (ctx.vditorInstance && mathValue) {
			const cleanMath = mathToKatex(mathValue);
			if (mathError && !confirm(`This formula may not render in your note:\n\n${mathError}\n\nInsert it anyway?`)) return;
			ctx.vditorInstance.insertValue(`\n$$\n${cleanMath}\n$$\n`);
		}
		mathDialog?.close();
	}

	return {
		mathToKatex,
		openMathDialog,
		insertMath,
		get mathDialog() { return mathDialog; },
		set mathDialog(value) { mathDialog = value; },
		get mathValue() { return mathValue; },
		set mathValue(value) { mathValue = value; },
		get mathLiveReady() { return mathLiveReady; },
		set mathLiveReady(value) { mathLiveReady = value; },
		get katexRenderer() { return katexRenderer; },
		set katexRenderer(value) { katexRenderer = value; },
		get mathError() { return mathError; },
		set mathError(value) { mathError = value; }
	};
}
