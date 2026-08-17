import type { StorageIssue } from '$lib/types';

export function readBrowserStorage(
	key: string,
	report: (issue: StorageIssue, error: unknown) => void
): string | null {
	try {
		return localStorage.getItem(key);
	} catch (error) {
		const issue: StorageIssue = {
			code: 'browser-storage',
			severity: 'warning',
			path: key,
			message:
				'A browser preference could not be saved; the rest of the library remains available.',
			recoverable: true
		};
		report(issue, error);
		return null;
	}
}
