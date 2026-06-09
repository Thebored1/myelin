export type Backlink = {
	sourceId: string;
	sourceTitle: string;
	targetBlock: string | null;
	contextExcerpt: string;
};

export type NoteSummary = {
	id: string;
	title: string;
	tags: string[];
	folder: string;
	excerpt: string;
	relativePath: string;
	createdAt: string;
	updatedAt: string;
	sourcePdf?: string | null;
	backlinks: Backlink[];
};

export type NoteDocument = {
	id: string;
	title: string;
	tags: string[];
	body: string;
	relativePath: string;
	createdAt: string;
	updatedAt: string;
	sourcePdf?: string | null;
	backlinks: Backlink[];
	annotations: PdfAnnotation[];
};

export type PdfAnnotation = {
	id: string;
	page: number;
	type: 'highlight' | 'draw' | 'image_extract' | 'text_highlight';
	points?: [number, number][];
	rect?: [number, number, number, number];
	rects?: [number, number, number, number][];
	color: string;
	strokeWidth: number;
};

export type ProviderStatus = {
	activeProvider: string;
	availableProviders: string[];
	healthy: boolean;
	detail: string;
};

export type IndexState = {
	isIndexing: boolean;
	lastIndexedAt: string | null;
	noteCount: number;
	backend: string;
};

export type AppSnapshot = {
	workspacePath: string | null;
	notes: NoteSummary[];
	customNoteOrder: string[];
	libraryFacets: {
		folders: string[];
		tags: string[];
	};
	providerStatus: ProviderStatus;
	indexState: IndexState;
};

export type SearchResult = {
	note: NoteSummary;
	score: number;
	reason: string;
};

export type SearchResponse = {
	query: string;
	results: SearchResult[];
};
