/** Remove model reasoning markers from content before it is shown in chat. */
export function hideThinkingContent(content: string): string {
	// An opening tag without a closing tag is still hidden while the model is
	// streaming. Attributes and casing are accepted because different models
	// emit slightly different forms of the marker.
	return content.replace(/<think\b[^>]*>[\s\S]*?(?:<\/think\s*>|$)/gi, '');
}
