import { invoke } from "@tauri-apps/api/core";
import type {
  AppTelemetryState,
  AppTelemetryUsage,
  AppUpdateStatus,
  CreateSessionRequest,
  HistoryEntry,
  MaintenanceReport,
  MemorySnapshot,
  PersistentSlotInfo,
  ProviderCredentialInput,
  ProviderUsage,
  SessionInfo,
  TokenTelemetry,
  UsageProviderId,
  VoiceServiceResponse,
} from "../types";

export function bytesToBase64(bytes: Uint8Array): string {
  let binary = "";
  const chunkSize = 0x8000;

  for (let offset = 0; offset < bytes.length; offset += chunkSize) {
    const chunk = bytes.subarray(offset, offset + chunkSize);
    binary += String.fromCharCode(...chunk);
  }

  return btoa(binary);
}

export function base64ToBytes(value: string): Uint8Array {
  const binary = atob(value);
  const bytes = new Uint8Array(binary.length);

  for (let index = 0; index < binary.length; index += 1) {
    bytes[index] = binary.charCodeAt(index);
  }

  return bytes;
}

export function createSession(request: CreateSessionRequest): Promise<SessionInfo> {
  return invoke("create_session", { request });
}

export function writeToSession(id: string, data: string): Promise<void> {
  return invoke("write_to_session", { id, data });
}

export function resizeSession(id: string, cols: number, rows: number): Promise<void> {
  return invoke("resize_session", { id, cols, rows });
}

export function scrollSession(id: string, lines: number): Promise<void> {
  return invoke("scroll_session", { id, lines });
}

export function detachSession(id: string): Promise<void> {
  return invoke("detach_session", { id });
}

export function closeSession(id: string): Promise<void> {
  return invoke("close_session", { id });
}

export function detachAllSessions(): Promise<void> {
  return invoke("detach_all_sessions");
}

export function closeAllSessions(): Promise<void> {
  return invoke("close_all_sessions");
}

export function listSessions(): Promise<SessionInfo[]> {
  return invoke("list_sessions");
}

export function listPersistentSlots(): Promise<PersistentSlotInfo[]> {
  return invoke("list_persistent_slots");
}

export function removePersistentSlot(slot: number): Promise<void> {
  return invoke("remove_persistent_slot", { slot });
}

export function systemMetrics(): Promise<MemorySnapshot> {
  return invoke("system_metrics");
}

export function tokenTelemetry(): Promise<TokenTelemetry> {
  return invoke("token_telemetry");
}

export function providerUsage(): Promise<ProviderUsage[]> {
  return invoke("provider_usage");
}

export function saveProviderCredential(input: ProviderCredentialInput): Promise<ProviderUsage> {
  return invoke("save_provider_credential", { input });
}

export function removeProviderCredential(provider: UsageProviderId): Promise<void> {
  return invoke("remove_provider_credential", { provider });
}

export function searchHistory(query: string, limit = 20): Promise<HistoryEntry[]> {
  return invoke("search_history", { query, limit });
}

export function maintenanceReport(): Promise<MaintenanceReport | null> {
  return invoke("maintenance_report");
}

/**
 * Gracefully relaunch UltraTerm. Terminal sessions persist in tmux and
 * reattach automatically once the new instance boots.
 */
export function restartApp(): Promise<void> {
  return invoke("restart_app");
}

export function checkAppUpdate(): Promise<AppUpdateStatus> {
  return invoke("check_app_update");
}

/**
 * Downloads, verifies, and installs the latest release, then exits so a
 * detached helper can swap the bundle and relaunch. A rejected promise means
 * the install failed and the current version is still running untouched.
 */
export function installAppUpdate(): Promise<void> {
  return invoke("install_app_update");
}

export function appTelemetryState(): Promise<AppTelemetryState> {
  return invoke("app_telemetry_state");
}

export function setAppTelemetryConsent(enabled: boolean): Promise<void> {
  return invoke("set_app_telemetry_consent", { enabled });
}
/**
 * Fire-and-forget product telemetry. Launch and heartbeat carry no usage
 * fields; usage reports contain only fixed daily terminal/session counters.
 * Calls are no-ops unless the user has opted in.
 */
export function recordAppEvent(
  event: "launch" | "heartbeat" | "usage",
  data: AppTelemetryUsage = {},
): Promise<void> {
  return invoke("record_app_event", { event, data });
}

export function voiceHealth(): Promise<VoiceServiceResponse> {
  return invoke("voice_health");
}

export function startVoiceInput(): Promise<VoiceServiceResponse> {
  return invoke("start_voice_input");
}

export function finishVoiceInput(recordingId: string): Promise<VoiceServiceResponse> {
  return invoke("finish_voice_input", { recordingId });
}

export function voiceInputStatus(recordingId: string): Promise<VoiceServiceResponse> {
  return invoke("voice_input_status", { recordingId });
}

export function cancelVoiceInput(recordingId: string): Promise<VoiceServiceResponse> {
  return invoke("cancel_voice_input", { recordingId });
}
