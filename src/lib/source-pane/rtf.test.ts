import { describe, expect, it } from 'vitest';
import { rtfToHtml, rtfToText } from './rtf';

describe('rtf converter', () => {
	it('renders plain paragraphs', () => {
		expect(rtfToHtml('{\\rtf1\\ansi Hello\\par World}')).toContain('<p>Hello</p><p>World</p>');
	});

	it('applies scoped bold and italic', () => {
		const html = rtfToHtml('{\\rtf1 normal {\\b bold} after\\par}');
		expect(html).toContain('<b>bold</b>');
		expect(html).toContain('after');
		expect(html).not.toContain('<b>after');
	});

	it('applies italic and underline', () => {
		const html = rtfToHtml('{\\rtf1 {\\i slant} and {\\ul under}\\par}');
		expect(html).toContain('<i>slant</i>');
		expect(html).toContain('<u>under</u>');
	});

	it('decodes hex escapes', () => {
		expect(rtfToHtml("{\\rtf1 caf\\'e9}")).toContain('café');
	});

	it('decodes unicode escapes and skips fallbacks', () => {
		const html = rtfToHtml('{\\rtf1 \\u8364?euro}');
		expect(html).toContain('€');
		expect(html).not.toContain('?');
	});

	it('skips font tables and pictures', () => {
		const html = rtfToHtml('{\\rtf1{\\fonttbl{\\f0 Times}}before{\\pict{\\*\\picprop}}after\\par}');
		expect(html).toContain('before');
		expect(html).toContain('after');
		expect(html).not.toContain('Times');
	});

	it('extracts plain text for ingestion', () => {
		expect(rtfToText('{\\rtf1 {\\b Bold} plain\\par second}')).toBe('Bold plain second');
	});
});
