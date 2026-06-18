export type ChatMessage = {
	id?: string;
	role: string;
	content: string;
	isStreaming?: boolean;
	error?: boolean;
	tools?: { name: string; details: string }[];
	snapshotId?: string;
	snapshot?: NoteSnapshot;
	isApprovalRequest?: boolean;
	approvalId?: string;
	approvalTool?: string;
	approvalDetails?: string;
	approvalStatus?: 'pending' | 'approved' | 'rejected';
	startTime?: number;
	endTime?: number;
};

export type NoteSnapshot = {
	noteBody: string;
	draftTitle: string;
	draftTags: string;
	chatLength: number;
};

export type GitCommit = {
	hash: string;
	author: string;
	timestamp: string;
	message: string;
};

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
	chatHistory: ChatMessage[];
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
	config?: {
		executablePath?: string;
		modelPath?: string;
		contextSize?: number;
		gpuLayers?: number;
		threads?: number;
		temperature?: number;
		topP?: number;
		extraArgs?: string[];
		backendPreference?: 'auto' | 'cuda' | 'vulkan' | 'metal' | 'cpu';
		gpuDevice?: string;
		thinking?: boolean;
		maxTurns?: number;
	};
	resolved?: {
		executablePath: string;
		modelPath: string;
		host: string;
		port: number;
		contextSize: number;
		gpuLayers?: number;
		threads?: number;
		temperature: number;
		topP: number;
		extraArgs: string[];
		backend?: string;
	};
	/** Backend running or preferred: "cuda" | "vulkan" | "metal" | "cpu". */
	activeBackend?: string;
	/** Whether an NVIDIA GPU was detected on this machine. */
	nvidiaDetected?: boolean;
	/** Whether GPU acceleration is usable on this machine at all. */
	gpuAvailable?: boolean;
	/** GPU adapter names detected on this machine. */
	gpus?: string[];
	/** Backend builds installed: subset of "cuda" | "vulkan" | "metal" | "cpu". */
	installedBackends?: string[];
};

/** A compute device exposed by a backend, e.g. { id: "Vulkan0", name: "Intel UHD" }. */
export type LlamaDevice = {
	id: string;
	name: string;
	backend: string;
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
