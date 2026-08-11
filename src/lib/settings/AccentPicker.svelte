<script lang="ts">
	import { normalizeHex } from '$lib/theme/color';
	import type { ThemeMode } from '$lib/theme/types';

	const PRESET_ACCENTS: Record<ThemeMode, string[]> = {
		dark: [
			'#EF6F2E',
			'#F97316',
			'#E11D48',
			'#F43F5E',
			'#EC4899',
			'#A855F7',
			'#8B5CF6',
			'#6366F1',
			'#3B82F6',
			'#06B6D4',
			'#10B981',
			'#84CC16',
			'#EAB308'
		],
		light: [
			'#D9480F',
			'#E8590C',
			'#E03131',
			'#C2255C',
			'#9C36B5',
			'#7048E8',
			'#4263EB',
			'#1971C2',
			'#1098AD',
			'#0B7285',
			'#2F9E44',
			'#82C91E',
			'#F59F00'
		]
	};

	let {
		value = '#EF6F2E',
		mode = 'dark',
		onchange = () => {}
	}: { value?: string; mode?: ThemeMode; onchange?: (hex: string) => void } = $props();

	let hue = $state(0);
	let saturation = $state(0.72);
	let valueLevel = $state(0.85);
	let hexInput = $state('');
	let hexInvalid = $state(false);
	let svBox = $state<HTMLElement | null>(null);
	let hueBar = $state<HTMLElement | null>(null);
	let swatchStrip = $state<HTMLElement | null>(null);

	function parseHex(hex: string): [number, number, number] {
		const normalized = normalizeHex(hex);
		return [
			parseInt(normalized.slice(1, 3), 16),
			parseInt(normalized.slice(3, 5), 16),
			parseInt(normalized.slice(5, 7), 16)
		];
	}

	function hexToHsv(hex: string): { h: number; s: number; v: number } {
		const [r, g, b] = parseHex(hex).map((channel) => channel / 255);
		const max = Math.max(r, g, b);
		const min = Math.min(r, g, b);
		const delta = max - min;
		let h = 0;
		if (delta > 0) {
			if (max === r) h = 60 * (((g - b) / delta) % 6);
			else if (max === g) h = 60 * ((b - r) / delta + 2);
			else h = 60 * ((r - g) / delta + 4);
		}
		return { h: (h + 360) % 360, s: max === 0 ? 0 : delta / max, v: max };
	}

	function hsvToHex(h: number, s: number, v: number): string {
		const chroma = v * s;
		const x = chroma * (1 - Math.abs(((h / 60) % 2) - 1));
		const m = v - chroma;
		let rgb: [number, number, number];
		if (h < 60) rgb = [chroma, x, 0];
		else if (h < 120) rgb = [x, chroma, 0];
		else if (h < 180) rgb = [0, chroma, x];
		else if (h < 240) rgb = [0, x, chroma];
		else if (h < 300) rgb = [x, 0, chroma];
		else rgb = [chroma, 0, x];
		return `#${rgb
			.map((channel) => Math.round((channel + m) * 255).toString(16).padStart(2, '0'))
			.join('')
			.toUpperCase()}`;
	}

	function syncFromValue() {
		let normalized: string;
		try {
			normalized = normalizeHex(value);
		} catch {
			return;
		}
		const hsv = hexToHsv(normalized);
		hue = hsv.h;
		saturation = hsv.s;
		valueLevel = hsv.v;
		hexInput = normalized;
		hexInvalid = false;
	}

	$effect(() => {
		syncFromValue();
	});

	function emitColor() {
		onchange(hsvToHex(hue, saturation, valueLevel));
	}

	function onSvPointer(event: PointerEvent) {
		const box = svBox;
		if (!box) return;
		box.setPointerCapture(event.pointerId);
		positionFromPointer(event);
	}

	function positionFromPointer(event: PointerEvent) {
		const box = svBox;
		if (!box) return;
		const rect = box.getBoundingClientRect();
		saturation = Math.max(0, Math.min(1, (event.clientX - rect.left) / rect.width));
		valueLevel = Math.max(0, Math.min(1, 1 - (event.clientY - rect.top) / rect.height));
		emitColor();
	}

	function onHuePointer(event: PointerEvent) {
		const bar = hueBar;
		if (!bar) return;
		bar.setPointerCapture(event.pointerId);
		hueFromPointer(event);
	}

	function hueFromPointer(event: PointerEvent) {
		const bar = hueBar;
		if (!bar) return;
		const rect = bar.getBoundingClientRect();
		hue = Math.max(0, Math.min(360, ((event.clientX - rect.left) / rect.width) * 360));
		emitColor();
	}

	function onSvKey(event: KeyboardEvent) {
		const step = event.shiftKey ? 0.2 : 0.05;
		if (event.key === 'ArrowUp') {
			valueLevel = Math.min(1, valueLevel + step);
			event.preventDefault();
		} else if (event.key === 'ArrowDown') {
			valueLevel = Math.max(0, valueLevel - step);
			event.preventDefault();
		} else if (event.key === 'ArrowRight') {
			saturation = Math.min(1, saturation + step);
			event.preventDefault();
		} else if (event.key === 'ArrowLeft') {
			saturation = Math.max(0, saturation - step);
			event.preventDefault();
		} else return;
		emitColor();
	}

	function onHueKey(event: KeyboardEvent) {
		const step = event.shiftKey ? 10 : 1;
		if (event.key === 'ArrowRight') {
			hue = (hue + step) % 360;
			event.preventDefault();
		} else if (event.key === 'ArrowLeft') {
			hue = (hue - step + 360) % 360;
			event.preventDefault();
		} else return;
		emitColor();
	}

	function onHexInput(event: Event) {
		const raw = (event.currentTarget as HTMLInputElement).value;
		hexInput = raw;
		if (!/^#[0-9a-fA-F]{6}([0-9a-fA-F]{2})?$/.test(raw)) {
			hexInvalid = raw.length > 0;
			return;
		}
		try {
			onchange(normalizeHex(raw));
			hexInvalid = false;
		} catch {
			hexInvalid = true;
		}
	}

	function scrollSwatches(direction: 1 | -1) {
		swatchStrip?.scrollBy({ left: direction * 140, behavior: 'smooth' });
	}

	const accent = $derived(hsvToHex(hue, saturation, valueLevel));
	const presets = $derived(PRESET_ACCENTS[mode]);
</script>

<div class="accent-picker">
	<div class="picker-row">
		<div
			bind:this={svBox}
			class="sv-box"
			role="slider"
			tabindex="0"
			aria-label="Saturation and lightness"
			aria-valuemin="0"
			aria-valuemax="100"
			aria-valuenow={Math.round(valueLevel * 100)}
			aria-valuetext={`saturation ${Math.round(saturation * 100)}%, lightness ${Math.round(valueLevel * 100)}%`}
			style="--hue-color: {hsvToHex(hue, 1, 1)}; --sv-x: {saturation * 100}%; --sv-y: {(1 - valueLevel) * 100}%;"
			onpointerdown={onSvPointer}
			onpointermove={(event) => {
				if (event.buttons === 1) positionFromPointer(event);
			}}
			onkeydown={onSvKey}
		>
			<span class="sv-thumb" aria-hidden="true"></span>
		</div>
		<div class="picker-column">
			<div
				bind:this={hueBar}
				class="hue-bar"
				role="slider"
				tabindex="0"
				aria-label="Hue"
				aria-valuenow={Math.round(hue)}
				aria-valuemin="0"
				aria-valuemax="360"
				style="--hue-pos: {hue / 360 * 100}%;"
				onpointerdown={onHuePointer}
				onpointermove={(event) => {
					if (event.buttons === 1) hueFromPointer(event);
				}}
				onkeydown={onHueKey}
			>
				<span class="hue-thumb" aria-hidden="true"></span>
			</div>
			<div class="hex-row">
				<span class="preview-swatch" style="background:{accent};" aria-hidden="true"></span>
				<input
					class:invalid={hexInvalid}
					value={hexInput}
					oninput={onHexInput}
					spellcheck="false"
					aria-label="Hex color"
					maxlength="9"
				/>
			</div>
		</div>
	</div>
	<div class="swatch-strip">
		<button class="swatch-nav" onclick={() => scrollSwatches(-1)} aria-label="Previous swatches">‹</button>
		<div bind:this={swatchStrip} class="swatches">
			{#each presets as color}
				<button
					class="swatch"
					class:active={color === accent}
					style="background:{color};"
					title={color}
					aria-label={color}
					onclick={() => onchange(color)}
				></button>
			{/each}
		</div>
		<button class="swatch-nav" onclick={() => scrollSwatches(1)} aria-label="More swatches">›</button>
	</div>
</div>

<style>
	.accent-picker {
		display: grid;
		gap: 0.7rem;
	}
	.picker-row {
		display: grid;
		grid-template-columns: minmax(0, 1fr) minmax(0, 1fr);
		gap: 0.7rem;
	}
	.sv-box {
		position: relative;
		aspect-ratio: 1.15 / 1;
		border-radius: var(--radius-md);
		border: 1px solid var(--border-default);
		cursor: crosshair;
		touch-action: none;
		background:
			linear-gradient(to top, #000, transparent),
			linear-gradient(to right, #fff, var(--hue-color));
	}
	.sv-thumb {
		position: absolute;
		left: var(--sv-x);
		top: var(--sv-y);
		width: 0.8rem;
		height: 0.8rem;
		border-radius: 50%;
		border: 2px solid #fff;
		box-shadow: 0 0 0 1px rgba(0, 0, 0, 0.55);
		transform: translate(-50%, -50%);
		pointer-events: none;
	}
	.picker-column {
		display: grid;
		gap: 0.7rem;
	}
	.hue-bar {
		position: relative;
		height: 1.6rem;
		border-radius: var(--radius-md);
		border: 1px solid var(--border-default);
		cursor: ew-resize;
		touch-action: none;
		background: linear-gradient(
			to right,
			#f00 0%,
			#ff0 17%,
			#0f0 33%,
			#0ff 50%,
			#00f 67%,
			#f0f 83%,
			#f00 100%
		);
	}
	.hue-thumb {
		position: absolute;
		left: var(--hue-pos);
		top: 0;
		bottom: 0;
		width: 3px;
		transform: translateX(-50%);
		background: #fff;
		box-shadow: 0 0 0 1px rgba(0, 0, 0, 0.55);
		pointer-events: none;
	}
	.hex-row {
		display: flex;
		align-items: center;
		gap: 0.5rem;
	}
	.preview-swatch {
		width: 1.5rem;
		height: 1.5rem;
		border-radius: var(--radius-sm);
		border: 1px solid var(--border-default);
	}
	.hex-row input {
		flex: 1;
		min-width: 0;
		color: var(--text-primary);
		background: var(--bg-input);
		border: 1px solid var(--border-default);
		border-radius: var(--radius-md);
		padding: 0.4rem 0.5rem;
		font: 0.8rem/1.2 ui-monospace, SFMono-Regular, Menlo, monospace;
		letter-spacing: 0.03em;
		text-transform: uppercase;
	}
	.hex-row input:focus {
		outline: 2px solid var(--accent-200);
		border-color: var(--accent-200);
	}
	.hex-row input.invalid {
		border-color: var(--danger);
	}
	.swatch-strip {
		display: grid;
		grid-template-columns: auto 1fr auto;
		align-items: center;
		gap: 0.4rem;
	}
	.swatches {
		display: flex;
		gap: 0.4rem;
		overflow-x: auto;
		scrollbar-width: thin;
		padding: 0.15rem;
	}
	.swatch {
		flex: 0 0 auto;
		width: 1.7rem;
		height: 1.7rem;
		border-radius: 50%;
		border: 1px solid var(--border-default);
		cursor: pointer;
		padding: 0;
	}
	.swatch:hover {
		transform: scale(1.1);
	}
	.swatch.active {
		box-shadow: 0 0 0 2px var(--bg-panel), 0 0 0 4px var(--accent-100);
	}
	.swatch-nav {
		color: var(--text-secondary);
		background: var(--bg-elevated);
		border: 1px solid var(--border-default);
		border-radius: var(--radius-md);
		width: 1.7rem;
		height: 1.7rem;
		font-size: 1rem;
		line-height: 1;
		cursor: pointer;
	}
	.swatch-nav:hover {
		color: var(--text-primary);
		border-color: var(--accent-200);
	}
	@media (max-width: 600px) {
		.picker-row {
			grid-template-columns: 1fr;
		}
	}
</style>
