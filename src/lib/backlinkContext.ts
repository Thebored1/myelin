function escapeHtml(value: string): string {
	return value.replace(/[&<>"']/g, (character) => {
		switch (character) {
			case '&':
				return '&amp;';
			case '<':
				return '&lt;';
			case '>':
				return '&gt;';
			case '"':
				return '&quot;';
			default:
				return '&#39;';
		}
	});
}

/**
 * Format a backlink excerpt using only markup generated here.
 *
 * Note content is escaped before the small Markdown subset is expanded. This
 * makes the returned string safe for Svelte's `{@html}` sink while preserving
 * the existing bold, italic, link-label, and block-link presentation.
 */
export function formatBacklinkContext(context: string): string {
	if (!context) return '';

	let html = escapeHtml(context);
	html = html.replace(/\[([^\]]+)\]\([^)]+\)/g, '<span class="backlink-link-label">$1</span>');
	html = html.replace(
		/\(\([a-fA-F0-9]{6}\)\)/g,
		'<span class="backlink-block-label">(Block Link)</span>'
	);
	html = html.replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>');
	html = html.replace(/\*([^*]+)\*/g, '<em>$1</em>');
	return html;
}
