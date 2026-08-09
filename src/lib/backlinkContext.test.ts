import { describe, expect, it } from 'vitest';
import { formatBacklinkContext } from './backlinkContext';

describe('formatBacklinkContext', () => {
	it('preserves the supported backlink formatting', () => {
		expect(formatBacklinkContext('See [Note](myelin://note), **bold**, *italic*, ((a1B2c3))')).toBe(
			'See <span class="backlink-link-label">Note</span>, <strong>bold</strong>, <em>italic</em>, <span class="backlink-block-label">(Block Link)</span>'
		);
	});

	it('escapes tags and attributes from note content before formatting', () => {
		const formatted = formatBacklinkContext(
			'<img src=x onerror="window.__pwned=true"> [<svg onload=alert(1)>](javascript:alert(1))'
		);

		expect(formatted).not.toContain('<img');
		expect(formatted).not.toContain('<svg');
		expect(formatted).not.toContain('javascript:');
		expect(formatted).toContain('&lt;img src=x onerror=&quot;window.__pwned=true&quot;&gt;');
		expect(formatted).toContain(
			'<span class="backlink-link-label">&lt;svg onload=alert(1)&gt;</span>'
		);
	});

	it('does not allow encoded markup to become active markup', () => {
		expect(formatBacklinkContext('&lt;script&gt;alert(1)&lt;/script&gt;')).toBe(
			'&amp;lt;script&amp;gt;alert(1)&amp;lt;/script&amp;gt;'
		);
	});
});
