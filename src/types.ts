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

/** Discovered OMP profile under $HOME/.omp/profiles. */
export interface OmpProfileInfo {
  name: string;
  active: boolean;
}

export type OmpThinkingLevel =
  | "off"
  | "minimal"
  | "low"
  | "medium"
  | "high"
  | "xhigh"
  | "max"
  | "auto";

export const OMP_THINKING_LEVELS: ReadonlyArray<OmpThinkingLevel> = [
  "off",
  "minimal",
  "low",
  "medium",
  "high",
  "xhigh",
  "max",
  "auto",
];

export interface CreateOmpProfileRequest {
  name: string;
  model: string;
  thinkingLevel: OmpThinkingLevel;
  titleModel?: string;
}

/**
 * A launch profile is an arbitrary OMP profile name; null means Default OMP
 * (the unprofiled installation, launched without a profile).
 */
export interface LaunchProfileOption {
  id: string | null;
  label: string;
  description: string;
}

/** Launch menu entries: Default OMP first, then every discovered profile. */
export function launchProfileOptions(profiles: ReadonlyArray<OmpProfileInfo>): LaunchProfileOption[] {
  return [
    {
      id: null,
      label: "Default OMP",
      description: "Launch OMP without a profile",
    },
    ...profiles.map((profile) => ({
      id: profile.name,
      label: profile.name,
      description: `Launch with the ${profile.name} OMP profile`,
    })),
  ];
}

export function launchProfileLabel(profile: string | null | undefined): string {
  return profile ? profile : "Default OMP";
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
 * profile. Arbitrary recorded names restore unchanged; empty or missing
 * metadata means Default OMP.
 */
export function launchProfileFromOmpProfile(profile: string | null | undefined): string | null {
  return profile ? profile : null;
}

export interface SessionInfo {
  id: string;
  slot: number;
  title: string;
  pid: number | null;
  launchedOmp: boolean;
  /** Recorded OMP profile name; null means Default OMP. */
  launchProfile: string | null;
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

export type UsageProviderId = "kimi" | "codex" | "codex-fallback" | "claude" | "zai";
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
  /**
   * Codex fallback accounts only: the full OAuth token set parsed from the
   * account's ~/.codex/auth.json, so OMP can keep refreshing the credential
   * after the pasted access token expires.
   */
  refreshToken?: string;
  /** Access-token expiry in epoch milliseconds. */
  expiresAt?: number;
  email?: string;
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
  /** Null or omitted launches Default OMP (no profile). */
  launchProfile?: string | null;
}

export interface TerminalOutputEvent {
  id: string;
  data: string;
}

export interface TerminalExitEvent {
  id: string;
}

/** utp `message` delivery: an explicitly addressed note between terminals. */
export interface TerminalMessageEvent {
  to: string;
  toSlot: number;
  fromSlot: number;
  text: string;
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
