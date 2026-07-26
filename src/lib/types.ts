export type Protocol = "responses" | "chat_completions";

export interface ModelConfig {
	id: string;
	name: string;
	enabled: boolean;
	contextWindow: number;
}

export interface Provider {
	id: string;
	name: string;
	baseUrl: string;
	apiKey: string;
	protocol: Protocol;
	enabled: boolean;
	models: ModelConfig[];
}

export interface Mapping {
	id: string;
	codexModel: string;
	providerId: string;
	upstreamModel: string;
	enabled: boolean;
}

export interface Settings {
	bindAddress: string;
	port: number;
	requestTimeoutSeconds: number;
	maxBodyMb: number;
	restartCodexOnChange: boolean;
	startOnLaunch: boolean;
}

export interface AppConfig {
	providers: Provider[];
	mappings: Mapping[];
	settings: Settings;
}

export interface Status {
	running: boolean;
	address: string;
	startedAt: number | null;
	requestCount: number;
	errorCount: number;
	configInjected: boolean;
	configPath: string;
	backupPath: string | null;
}

export interface LogEntry {
	id: number;
	timestamp: number;
	level: "INFO" | "WARN" | "ERROR";
	message: string;
}
