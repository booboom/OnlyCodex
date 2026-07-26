import { invoke } from "@tauri-apps/api/core";
import type { AppConfig, LogEntry, ModelConfig, Status } from "./types";

const browserFallback: AppConfig = {
	providers: [],
	mappings: [],
	settings: {
		bindAddress: "127.0.0.1",
		port: 8787,
		requestTimeoutSeconds: 120,
		maxBodyMb: 20,
		restartCodexOnChange: true,
		startOnLaunch: false,
	},
};

const isTauri = () => "__TAURI_INTERNALS__" in window;

export async function loadConfig(): Promise<AppConfig> {
	return isTauri() ? invoke("get_config") : structuredClone(browserFallback);
}
export async function saveConfig(config: AppConfig): Promise<AppConfig> {
	if (!isTauri()) {
		Object.assign(browserFallback, structuredClone(config));
		return config;
	}
	return invoke("save_config", { config });
}
export async function getStatus(): Promise<Status> {
	if (!isTauri())
		return {
			running: false,
			address: "127.0.0.1:8787",
			startedAt: null,
			requestCount: 0,
			errorCount: 0,
			configInjected: false,
			configPath: "~/.codex/config.toml",
			backupPath: null,
		};
	return invoke("get_status");
}
export const startProxy = (): Promise<Status> => invoke("start_proxy");
export const stopProxy = (): Promise<Status> => invoke("stop_proxy");
export const restoreCodex = (): Promise<Status> =>
	invoke("restore_codex_config");
export const openCodexConfig = (): Promise<void> => invoke("open_codex_config");
export const testProvider = (providerId: string): Promise<ModelConfig[]> =>
	invoke("test_provider", { providerId });
export const getLogs = (): Promise<LogEntry[]> =>
	isTauri() ? invoke("get_logs") : Promise.resolve([]);
export const clearLogs = (): Promise<void> =>
	isTauri() ? invoke("clear_logs") : Promise.resolve();
export const revealDataDir = (): Promise<void> => invoke("reveal_data_dir");
export const backupCodex = (): Promise<Status> => invoke("backup_codex_config");
export const refreshCatalog = (): Promise<number> =>
	invoke("refresh_model_catalog");
export const restartCodex = (): Promise<void> => invoke("restart_codex");
