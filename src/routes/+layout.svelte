<script lang="ts">
	import favicon from '$lib/assets/favicon.svg';

	let { children } = $props();
</script>

<svelte:head>
	<link rel="icon" href={favicon} />
	<link rel="preconnect" href="https://fonts.googleapis.com" />
	<link rel="preconnect" href="https://fonts.gstatic.com" crossorigin="anonymous" />
	<link
		href="https://fonts.googleapis.com/css2?family=Inter:wght@300;400;500;600;700&family=JetBrains+Mono:wght@400;500&display=swap"
		rel="stylesheet"
	/>
</svelte:head>

{@render children()}

<style>
	:global(:root) {
		--accent-100: #ef6f2e;
		--accent-200: #ee6018;
		--accent-300: #d15010;
		--surface-dark-primary: #020202;
		--surface-dark-secondary: #101010;
		--surface-light-primary: #eeeeee;
		--surface-light-secondary: #fafafa;
		--neutral-100: #d6d3d2;
		--neutral-200: #ccc9c7;
		--neutral-300: #b8b3b0;
		--neutral-400: #a49d9a;
		--neutral-500: #8a8380;
		--neutral-600: #5c5855;
		--neutral-700: #4d4947;
		--neutral-800: #3d3a39;
		--neutral-900: #2e2c2b;
		--neutral-1000: #1f1d1c;
		--text-primary: #eeeeee;
		--text-secondary: #a49d9a;
		--text-inverse: #020202;
		--text-hero: #f6f1e7;
		--border-default: #3d3a39;
		--border-subtle: #4d4947;
		--bg-page: #020202;
		--bg-panel: #101010;
		--bg-code: #1f1d1c;
		--bg-selection: #ef6f2e;
		--font-sans: 'Inter', system-ui, -apple-system, sans-serif;
		--font-mono: 'JetBrains Mono', 'Cascadia Code', 'Fira Code', monospace;
		--space-1: 0.25rem;
		--space-2: 0.5rem;
		--space-3: 0.75rem;
		--space-4: 1rem;
		--space-5: 1.25rem;
		--space-6: 1.5rem;
		--space-8: 2rem;
		--space-10: 2.5rem;
		--space-12: 3rem;
		--space-16: 4rem;
		--space-20: 5rem;
		--radius-xs: 0.125rem;
		--radius-sm: 0.1875rem;
		--radius-md: 0.25rem;
		--radius-lg: 0.375rem;
		--radius-xl: 0.5rem;
		--radius-2xl: 0.625rem;
		--radius-3xl: 0.75rem;
		--duration-fast: 0.15s;
		--duration-page: 0.32s;
		--duration-content: 0.64s;
		--ease-standard: cubic-bezier(0.4, 0, 0.2, 1);
		--ease-out: cubic-bezier(0, 0, 0.2, 1);
		--ease-emphasis: cubic-bezier(0.22, 1, 0.36, 1);
		--blur-sm: 8px;
		--blur-md: 12px;
		--blur-xl: 24px;
	}

	:global(html) {
		background: var(--bg-page);
		color: var(--text-primary);
	}

	:global(body) {
		margin: 0;
		font-family: var(--font-sans);
		background:
			radial-gradient(circle at top right, rgba(239, 111, 46, 0.12), transparent 20rem),
			linear-gradient(180deg, #050505 0%, #020202 100%);
		color: var(--text-primary);
		-webkit-font-smoothing: antialiased;
	}

	:global(*),
	:global(*::before),
	:global(*::after) {
		box-sizing: border-box;
	}

	:global(::selection) {
		background: var(--bg-selection);
		color: var(--text-inverse);
	}
</style>
