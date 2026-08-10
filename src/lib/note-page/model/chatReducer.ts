import type { ChatMessage } from '$lib/types';

export type ChatEvent =
	| { type: 'chunk'; requestId: string; delta: string }
	| { type: 'tool'; tool: string; details: string; mutatesNote?: boolean }
	| { type: 'done'; requestId: string; tools?: { name: string; details: string }[] }
	| { type: 'error'; requestId: string; message: string; tools?: { name: string; details: string }[] };

export function reduceChatEvent(
	messages: ChatMessage[],
	event: ChatEvent,
	activeRequestId: string | null
): ChatMessage[] {
	if ((event.type === 'chunk' || event.type === 'done' || event.type === 'error') && event.requestId !== activeRequestId) {
		return messages;
	}
	if (event.type === 'chunk') {
		return messages.map((message) =>
			message.isStreaming
				? { ...message, content: message.content + event.delta, statusText: undefined }
				: message
		);
	}
	if (event.type === 'tool') {
		const previous = messages.map((message) =>
			message.isStreaming
				? { ...message, isStreaming: false, content: event.mutatesNote ? '' : message.content }
				: message
		);
		return [
			...previous,
			{ role: 'assistant', content: '', tools: [{ name: event.tool, details: event.details }] },
			{ role: 'assistant', content: '', isStreaming: true }
		];
	}
	if (event.type === 'done') {
		return messages.map((message) =>
			message.isStreaming ? { ...message, isStreaming: false, tools: event.tools ?? message.tools } : message
		);
	}
	return messages.map((message) =>
		message.isStreaming
			? { ...message, isStreaming: false, error: true, content: event.message, tools: event.tools ?? message.tools }
			: message
	);
}

