import { describe, expect, it } from 'vitest';
import { hideThinkingContent } from './chatContent';

describe('hideThinkingContent', () => {
	it('removes empty and complete thinking blocks', () => {
		expect(hideThinkingContent('<think></think>')).toBe('');
		expect(hideThinkingContent('<think>reasoning</think>Answer')).toBe('Answer');
	});

	it('handles case-insensitive markers and attributes', () => {
		expect(hideThinkingContent('<THINK mode="hidden">reasoning</THINK>Answer')).toBe('Answer');
	});

	it('hides an unfinished block while streaming', () => {
		expect(hideThinkingContent('before<think>still reasoning')).toBe('before');
	});

	it('preserves normal content around thinking blocks', () => {
		expect(hideThinkingContent('One\n<think>internal</think>\n**Two**')).toBe('One\n\n**Two**');
	});
});
