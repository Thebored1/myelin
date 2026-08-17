<script lang="ts">
	import type { ScrollMode, SpreadMode, ToolMode } from '../types';
	let {
		toolMode = $bindable<ToolMode>('select'),
		scrollMode = $bindable<ScrollMode>('page'),
		spreadMode = $bindable<SpreadMode>('none'),
		showMoreMenu = $bindable(false),
		showAttachButton = true,
		onAttachNote
	}: {
		toolMode?: ToolMode;
		scrollMode?: ScrollMode;
		spreadMode?: SpreadMode;
		showMoreMenu?: boolean;
		showAttachButton?: boolean;
		onAttachNote?: () => void;
	} = $props();
</script>

<div class="more-menu-container">
	<button
		class="more-btn"
		class:has-active-tool={toolMode !== 'select'}
		onclick={(e) => {
			e.stopPropagation();
			showMoreMenu = !showMoreMenu;
		}}
		title="More Tools"
	>
		<svg
			viewBox="0 0 24 24"
			width="16"
			height="16"
			fill="currentColor"
			stroke="none"
			xmlns="http://www.w3.org/2000/svg"
			><circle cx="5" cy="12" r="1.5"></circle><circle cx="12" cy="12" r="1.5"></circle><circle
				cx="19"
				cy="12"
				r="1.5"
			></circle></svg
		>
	</button>
	{#if showMoreMenu}
		<div class="dropdown more-dropdown">
			{#if showAttachButton && onAttachNote}
				<div class="menu-group">
					<button
						class="attach-btn-inline"
						onclick={() => {
							onAttachNote!();
							showMoreMenu = false;
						}}
					>
						<svg
							viewBox="0 0 24 24"
							width="14"
							height="14"
							stroke="currentColor"
							stroke-width="2"
							fill="none"
							><path d="M12 20h9"></path><path
								d="M16.5 3.5a2.121 2.121 0 0 1 3 3L7 19l-4 1 1-4L16.5 3.5z"
							></path></svg
						>
						Attach Note
					</button>
				</div>
				<div class="menu-divider"></div>
			{/if}
			<div class="menu-group">
				<button
					class:active={toolMode === 'select'}
					onclick={() => {
						toolMode = 'select';
						showMoreMenu = false;
					}}
				>
					<svg
						viewBox="0 0 24 24"
						width="14"
						height="14"
						stroke="currentColor"
						stroke-width="2"
						fill="none"
						><path d="M3 3l7.07 16.97 2.51-7.39 7.39-2.51L3 3z"></path><path d="M13 13l6 6"
						></path></svg
					>
					Select
				</button>
				<button
					class:active={toolMode === 'pen'}
					onclick={() => {
						toolMode = 'pen';
						showMoreMenu = false;
					}}
				>
					<svg
						viewBox="0 0 24 24"
						width="14"
						height="14"
						stroke="currentColor"
						stroke-width="2"
						fill="none"
						><path d="M12 19l7-7 3 3-7 7-3-3z"></path><path
							d="M18 13l-1.5-7.5L2 2l3.5 14.5L13 18l5-5z"
						></path><path d="M2 2l7.586 7.586"></path><circle cx="11" cy="11" r="2"></circle></svg
					>
					Draw
				</button>
				<button
					class:active={toolMode === 'eraser'}
					onclick={() => {
						toolMode = 'eraser';
						showMoreMenu = false;
					}}
				>
					<svg
						viewBox="0 0 24 24"
						width="14"
						height="14"
						stroke="currentColor"
						stroke-width="2"
						fill="none"
						><path
							d="M20 20H7L3 16C2.5 15.5 2.5 14.5 3 14L13 4C13.5 3.5 14.5 3.5 15 4L20 9C20.5 9.5 20.5 10.5 20 11L11 20H20"
						></path></svg
					>
					Eraser
				</button>
				<button
					class:active={toolMode === 'marquee'}
					onclick={() => {
						toolMode = 'marquee';
						showMoreMenu = false;
					}}
				>
					<svg
						viewBox="0 0 24 24"
						width="14"
						height="14"
						stroke="currentColor"
						stroke-width="2"
						fill="none"
						><rect x="3" y="3" width="18" height="18" rx="2" ry="2" stroke-dasharray="4 4"
						></rect></svg
					>
					Marquee
				</button>
			</div>
			<div class="menu-divider"></div>
			<div class="menu-group">
				<button
					class:active={scrollMode === 'page' && spreadMode === 'none'}
					onclick={() => {
						scrollMode = 'page';
						spreadMode = 'none';
						showMoreMenu = false;
					}}
				>
					<svg
						viewBox="0 0 24 24"
						width="14"
						height="14"
						stroke="currentColor"
						stroke-width="2"
						fill="none"
						><rect x="7" y="3" width="10" height="18" rx="2"></rect><path d="M7 8h10M7 16h10"
						></path></svg
					>
					Single Page
				</button>
				<button
					class:active={scrollMode === 'vertical'}
					onclick={() => {
						scrollMode = 'vertical';
						showMoreMenu = false;
					}}
				>
					<svg
						viewBox="0 0 24 24"
						width="14"
						height="14"
						stroke="currentColor"
						stroke-width="2"
						fill="none"
						><rect x="7" y="3" width="10" height="18" rx="2"></rect><path d="M12 3v18"></path></svg
					>
					Vertical Scrolling
				</button>
				<button
					class:active={scrollMode === 'horizontal'}
					onclick={() => {
						scrollMode = 'horizontal';
						showMoreMenu = false;
					}}
				>
					<svg
						viewBox="0 0 24 24"
						width="14"
						height="14"
						stroke="currentColor"
						stroke-width="2"
						fill="none"
						><rect x="3" y="7" width="18" height="10" rx="2"></rect><path d="M3 12h18"></path></svg
					>
					Horizontal Scrolling
				</button>
				<button
					class:active={scrollMode === 'wrapped'}
					onclick={() => {
						scrollMode = 'wrapped';
						showMoreMenu = false;
					}}
				>
					<svg
						viewBox="0 0 24 24"
						width="14"
						height="14"
						stroke="currentColor"
						stroke-width="2"
						fill="none"
						><rect x="3" y="3" width="7" height="7" rx="1"></rect><rect
							x="14"
							y="3"
							width="7"
							height="7"
							rx="1"
						></rect><rect x="3" y="14" width="7" height="7" rx="1"></rect><rect
							x="14"
							y="14"
							width="7"
							height="7"
							rx="1"
						></rect></svg
					>
					Wrapped Scrolling
				</button>
			</div>
		</div>
	{/if}
</div>
