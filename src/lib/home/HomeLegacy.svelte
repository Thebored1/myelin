<script lang="ts">
import HomeRail from '$lib/home/components/HomeRail.svelte';
import HomeMain from '$lib/home/components/HomeMain.svelte';
import HomeDialogs from '$lib/home/components/HomeDialogs.svelte';
import '$lib/home/home.css';
import { sidebarOpen } from '$lib/stores';
import { createHomeController } from '$lib/home/controller.svelte';

const home = createHomeController();
</script>

<svelte:head><title>myelin</title></svelte:head>
<svelte:window
	onclick={() => {
		home.activeMenuId = null;
		home.showAddMenu = false;
		home.showNotebookMenu = false;
	}}
/>
<HomeDialogs {home} />

{#if home.storageIssues.length}
	<div class="storage-warning" role="alert">
		<strong>Some saved data needs attention ({home.storageIssues.length})</strong>
		{#each home.storageIssues.slice(0, 3) as issue}
			<div>{issue.path ? `${issue.path}: ` : ''}{issue.message}</div>
		{/each}
	</div>
{/if}

<div class="shell" class:rail-collapsed={!$sidebarOpen}>
	<HomeRail {home} />
	<HomeMain {home} />
</div>

<style>
	.storage-warning {
		position: fixed;
		top: 0.5rem;
		left: 50%;
		z-index: 20;
		max-width: min(42rem, calc(100vw - 2rem));
		padding: 0.6rem 0.9rem;
		transform: translateX(-50%);
		border: 1px solid #d99b32;
		border-radius: 0.45rem;
		background: #fff8e8;
		color: #6a4300;
		font-size: 0.78rem;
	}
</style>
