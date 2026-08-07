export type ThemeId = "oled" | "aurora" | "titanium" | "ember";
export type EffectMode = "off" | "ambient" | "focus" | "spectrum";
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

export type SessionStatus = "connecting" | "live" | "exited" | "error";
export type SessionActivity = "idle" | "working";

export type LaunchProfileId = "default" | "gpt-only" | "kimi-k3";

export interface LaunchProfileOption {
  id: LaunchProfileId;
  label: string;
  description: string;
}

export const LAUNCH_PROFILE_OPTIONS: ReadonlyArray<LaunchProfileOption> = [
  { id: "default", label: "Default", description: "Inherit the current OMP setup" },
  { id: "gpt-only", label: "GPT only", description: "Launch with the gpt-only OMP profile" },
  { id: "kimi-k3", label: "Kimi K3", description: "Launch with the kimi-k3 OMP profile" },
];

export function isLaunchProfileId(value: unknown): value is LaunchProfileId {
  return value === "default" || value === "gpt-only" || value === "kimi-k3";
}

export function launchProfileLabel(profile: LaunchProfileId | null | undefined): string {
  return LAUNCH_PROFILE_OPTIONS.find((option) => option.id === profile)?.label ?? "Default";
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

export interface TerminalTokenTelemetry {
  slot: number;
  sessionId: string | null;
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
  history: TokenHistoryDay[];
  past24Hours: TokenCounts;
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
