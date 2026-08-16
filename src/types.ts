export type ThemeId = "oled" | "white" | "aurora" | "titanium" | "ember";
export type TerminalCursorStyle = "bar" | "block" | "underline";

export interface TerminalPreferences {
  fontSize: number;
  cursorStyle: TerminalCursorStyle;
  cursorBlink: boolean;
}

export const DEFAULT_TERMINAL_PREFERENCES: Readonly<TerminalPreferences> = {
  fontSize: 10,
  cursorStyle: "bar",
  cursorBlink: true,
};

export interface ProviderUsagePreferences {
  showWeeklyPace: boolean;
  showResetTimes: boolean;
  showSecondaryWindows: boolean;
}

export const DEFAULT_PROVIDER_USAGE_PREFERENCES: Readonly<ProviderUsagePreferences> = {
  showWeeklyPace: true,
  showResetTimes: true,
  showSecondaryWindows: true,
};

export type SessionStatus = "connecting" | "live" | "exited" | "error";
export type SessionActivity = "idle" | "working";

export type LaunchProfileId = "default" | "gpt-only" | "kimi-k3" | "deepseek-v4-flash";

export interface LaunchProfileOption {
  id: LaunchProfileId;
  label: string;
  description: string;
}

export const LAUNCH_PROFILE_OPTIONS: ReadonlyArray<LaunchProfileOption> = [
  { id: "default", label: "Default", description: "Launch the mixed Kimi, Sol, and Luna OMP profile" },
  { id: "gpt-only", label: "GPT only", description: "Launch with the gpt-only OMP profile" },
  { id: "kimi-k3", label: "Kimi K3", description: "Launch with the kimi-k3 OMP profile" },
  {
    id: "deepseek-v4-flash",
    label: "DeepSeek V4 Flash",
    description: "Launch an all-DeepSeek V4 Flash OMP profile",
  },
];

export function isLaunchProfileId(value: unknown): value is LaunchProfileId {
  return value === "default"
    || value === "gpt-only"
    || value === "kimi-k3"
    || value === "deepseek-v4-flash";
}

export function launchProfileLabel(profile: LaunchProfileId | null | undefined): string {
  return LAUNCH_PROFILE_OPTIONS.find((option) => option.id === profile)?.label ?? "Default";
}

export interface PersistentSlotInfo {
  slot: number;
  profile: string | null;
}

export interface AppUpdateStatus {
  currentVersion: string;
  latestVersion: string;
  updateAvailable: boolean;
  releaseUrl: string;
}

export type AppTelemetryConsent = "unset" | "enabled" | "disabled";

export interface AppTelemetryState {
  consent: AppTelemetryConsent;
}
export interface AppTelemetryUsage {
  /** Successful terminal pane starts observed since the last report. */
  terminals?: number;
  /** Successful OMP session starts observed since the last report. */
  sessions?: number;
}

/**
 * Maps the OMP profile recorded on a persistent tmux session back to a launch
 * profile. Legacy sessions without metadata (and the default "lds" profile)
 * restore as "default".
 */
export function launchProfileFromOmpProfile(profile: string | null | undefined): LaunchProfileId {
  return isLaunchProfileId(profile) && profile !== "default" ? profile : "default";
}

export interface SessionInfo {
  id: string;
  slot: number;
  title: string;
  pid: number | null;
  launchedOmp: boolean;
  launchProfile: LaunchProfileId;
}

export interface WorkspaceSession extends SessionInfo {
  status: SessionStatus;
  activity: SessionActivity;
  error?: string;
}
export interface HistoryEntry {
  id: number;
  prompt: string;
  createdAt: number;
  cwd: string | null;
  sessionId: string | null;
  truncated: boolean;
}


export interface MemorySnapshot {
  appMemoryMib: number;
  terminalMemoryMib: number;
  sessionCount: number;
  maxSessions: number;
}

export interface TokenCounts {
  input: number;
  output: number;
  cacheRead: number;
  cacheWrite: number;
  total: number;
}

export interface TokenChannelTelemetry {
  subscription: TokenCounts;
  paidApi: TokenCounts;
  paidApiCostUsd: number;
}

export interface TerminalTokenTelemetry {
  slot: number;
  sessionId: string | null;
  title: string | null;
  model: string | null;
  usage: TokenCounts;
  activeSubagents: number;
  inactiveSubagents: number;
}

export interface TokenModelUsage {
  model: string;
  usage: TokenCounts;
}

export interface TokenHistoryDay {
  /** Local calendar day in YYYY-MM-DD form. */
  date: string;
  usage: TokenCounts;
  models: TokenModelUsage[];
}

export interface TokenTelemetry {
  terminals: TerminalTokenTelemetry[];
  /** Exact tokens for the current local calendar day. */
  today: TokenCounts;
  todayChannels: TokenChannelTelemetry;
  history: TokenHistoryDay[];
  past24Hours: TokenCounts;
  past24HourChannels: TokenChannelTelemetry;
  past7Days: TokenCounts;
  allTime: TokenCounts;
  activeSubagents: number;
  inactiveSubagents: number;
  parallelAgents: number;
  trackedSessions: number;
  updatedAt: number;
}

export type UsageProviderId = "kimi" | "codex" | "claude" | "zai";
export type ProviderUsageStatus = "connected" | "loading" | "stale" | "error" | "disconnected";

export interface ProviderUsageWindow {
  label: string;
  usedPercent: number;
  resetsAt: number | null;
}

export interface ProviderUsage {
  provider: UsageProviderId;
  displayName: string;
  plan: string | null;
  status: ProviderUsageStatus;
  windows: ProviderUsageWindow[];
  balance: string | null;
  updatedAt: number | null;
  error: string | null;
}

export interface ProviderCredentialInput {
  provider: UsageProviderId;
  accessToken: string;
  accountId?: string;
}

export interface MaintenanceTaskReport {
  name: string;
  status: string;
}

export interface MaintenanceReport {
  schemaVersion: number;
  status: string;
  startedAt: string | null;
  completedAt: string | null;
  localDate: string | null;
  reclaimedBytes: number;
  tasks: MaintenanceTaskReport[];
}

export type VoiceInputState =
  | "idle"
  | "connecting"
  | "recording"
  | "transcribing"
  | "preview";

export interface VoiceServiceResponse {
  version: number;
  requestId: string;
  ok: boolean;
  state: string;
  recordingId: string | null;
  transcript: string | null;
  error: string | null;
  serviceStarted: boolean;
  audioLevel: number | null;
}

export interface CreateSessionRequest {
  slot: number;
  cols: number;
  rows: number;
  workingDirectory?: string;
  launchOmp: boolean;
  /** Omitted means the backend launches the `default` profile. */
  launchProfile?: LaunchProfileId;
}

export interface TerminalOutputEvent {
  id: string;
  data: string;
}

export interface TerminalExitEvent {
  id: string;
}

export interface TerminalController {
  write(data: Uint8Array): void;
  focus(): void;
  fit(): void;
  scrollLines(lines: number): void;
  scrollPages(pages: number): void;
  scrollToBottom(): void;
  hasPendingInput(): boolean;
  isAlternateBuffer(): boolean;
  trackInput(data: string): void;
}

export const TERMINAL_SCROLLBACK = 5_000;
