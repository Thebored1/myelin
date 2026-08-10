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

<div class="shell" class:rail-collapsed={!$sidebarOpen}>
	<HomeRail {home} />
	<HomeMain {home} />
</div>
