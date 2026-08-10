<script lang="ts">
	import { createNotePageController } from '$lib/note-page/controller.svelte';
	import NoteWorkspace from '$lib/note-page/components/NoteWorkspace.svelte';
	import NoteDialogs from '$lib/note-page/components/NoteDialogs.svelte';
	import 'mathlive/fonts.css';
	import './note-page.css';

	const notePage = createNotePageController();
</script>

<svelte:head>
	<title>{notePage.note ? notePage.note.title : 'Myelin'}</title>
	<style>
		.vditor-sv::-webkit-scrollbar {
			display: none !important;
			width: 0 !important;
			background: transparent !important;
		}
		.vditor-sv {
			scrollbar-width: none !important;
			-ms-overflow-style: none !important;
		}
	</style>
</svelte:head>

<NoteWorkspace {notePage} />
<NoteDialogs {notePage} />
