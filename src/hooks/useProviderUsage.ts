import { useSyncExternalStore } from "react";
import {
  providerUsage,
  removeProviderCredential,
  saveProviderCredential,
} from "../lib/terminalApi";
import type {
  ProviderCredentialInput,
  ProviderUsage,
  ProviderUsageWindow,
  UsageProviderId,
} from "../types";

const POLL_INTERVAL_MS = 60_000;
const STALE_AFTER_MS = 150_000;

export const USAGE_PROVIDERS: ReadonlyArray<{ id: UsageProviderId; name: string }> = [
  { id: "kimi", name: "Kimi" },
  { id: "codex", name: "Codex" },
  { id: "codex-fallback", name: "Codex Fallback" },
  { id: "claude", name: "Claude" },
  { id: "zai", name: "ZAI" },
];

const PROVIDER_IDS = USAGE_PROVIDERS.map((provider) => provider.id);
const PROVIDER_NAMES: Record<UsageProviderId, string> = {
  kimi: "Kimi",
  codex: "Codex",
  "codex-fallback": "Codex Fallback",
  claude: "Claude",
  zai: "ZAI",
};

export interface ProviderUsageState {
  usages: ProviderUsage[];
  /** True until the first fetch settles. */
  loading: boolean;
  /** True while any fetch is in flight. */
  refreshing: boolean;
  /** Section-level failure (the whole provider_usage call failed). */
  error: string | null;
  lastFetchedAt: number | null;
}

function disconnectedUsage(provider: UsageProviderId): ProviderUsage {
  return {
    provider,
    displayName: PROVIDER_NAMES[provider],
    plan: null,
    status: "disconnected",
    windows: [],
    balance: null,
    updatedAt: null,
    error: null,
  };
}

function isProviderId(value: unknown): value is UsageProviderId {
  return typeof value === "string" && (PROVIDER_IDS as string[]).includes(value);
}

function clampPercent(value: unknown): number {
  const numeric = typeof value === "number" && Number.isFinite(value) ? value : 0;
  return Math.min(100, Math.max(0, numeric));
}

function normalizeWindow(window: ProviderUsageWindow): ProviderUsageWindow {
  return {
    label: typeof window?.label === "string" && window.label.trim() ? window.label : "Usage window",
    usedPercent: clampPercent(window?.usedPercent),
    resetsAt: typeof window?.resetsAt === "number" && Number.isFinite(window.resetsAt)
      ? window.resetsAt
      : null,
  };
}

function normalizeUsage(usage: ProviderUsage): ProviderUsage {
  return {
    ...usage,
    displayName: usage.displayName || PROVIDER_NAMES[usage.provider] || usage.provider,
    windows: Array.isArray(usage.windows) ? usage.windows.map(normalizeWindow) : [],
  };
}

function normalizeUsages(list: ProviderUsage[] | null | undefined): ProviderUsage[] {
  const byProvider = new Map<UsageProviderId, ProviderUsage>();
  for (const usage of Array.isArray(list) ? list : []) {
    if (usage && isProviderId(usage.provider)) byProvider.set(usage.provider, normalizeUsage(usage));
  }
  return PROVIDER_IDS.map((provider) => byProvider.get(provider) ?? disconnectedUsage(provider));
}

function errorMessage(error: unknown): string {
  if (typeof error === "string") return error;
  if (error instanceof Error && error.message) return error.message;
  return "Provider usage request failed";
}

let state: ProviderUsageState = {
  usages: PROVIDER_IDS.map(disconnectedUsage),
  loading: true,
  refreshing: false,
  error: null,
  lastFetchedAt: null,
};

const listeners = new Set<() => void>();
let subscriberCount = 0;
let pollTimer: number | null = null;
let usageGeneration = 0;

function setState(patch: Partial<ProviderUsageState>): void {
  state = { ...state, ...patch };
  listeners.forEach((listener) => listener());
}

async function refreshProviderUsage(): Promise<void> {
  const refreshGeneration = ++usageGeneration;
  setState({ refreshing: true });
  try {
    const list = await providerUsage();
    if (refreshGeneration !== usageGeneration) return;
    setState({
      usages: normalizeUsages(list),
      loading: false,
      refreshing: false,
      error: null,
      lastFetchedAt: Date.now(),
    });
  } catch (error) {
    if (refreshGeneration !== usageGeneration) return;
    setState({ loading: false, refreshing: false, error: errorMessage(error) });
  }
}

async function connectProvider(input: ProviderCredentialInput): Promise<ProviderUsage> {
  usageGeneration += 1;
  setState({ refreshing: false });
  const usage = normalizeUsage(await saveProviderCredential(input));
  usageGeneration += 1;
  setState({
    usages: state.usages.map((item) => (item.provider === usage.provider ? usage : item)),
    loading: false,
    refreshing: false,
    error: null,
    lastFetchedAt: Date.now(),
  });
  void refreshProviderUsage();
  return usage;
}

async function disconnectProvider(provider: UsageProviderId): Promise<void> {
  usageGeneration += 1;
  setState({ refreshing: false });
  await removeProviderCredential(provider);
  usageGeneration += 1;
  setState({
    usages: state.usages.map((item) => (item.provider === provider ? disconnectedUsage(provider) : item)),
    refreshing: false,
    error: null,
  });
  void refreshProviderUsage();
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  subscriberCount += 1;
  if (subscriberCount === 1) {
    if (state.lastFetchedAt === null) void refreshProviderUsage();
    pollTimer = window.setInterval(() => void refreshProviderUsage(), POLL_INTERVAL_MS);
  }
  return () => {
    listeners.delete(listener);
    subscriberCount -= 1;
    if (subscriberCount === 0 && pollTimer !== null) {
      window.clearInterval(pollTimer);
      pollTimer = null;
    }
  };
}

export function useProviderUsage(): ProviderUsageState & {
  refresh: () => Promise<void>;
  connect: (input: ProviderCredentialInput) => Promise<ProviderUsage>;
  disconnect: (provider: UsageProviderId) => Promise<void>;
} {
  const snapshot = useSyncExternalStore(subscribe, () => state);
  return {
    ...snapshot,
    refresh: refreshProviderUsage,
    connect: connectProvider,
    disconnect: disconnectProvider,
  };
}

/** Epoch timestamps arrive in milliseconds from the backend; tolerate seconds defensively. */
function toMillis(timestamp: number): number {
  return timestamp < 1_000_000_000_000 ? timestamp * 1000 : timestamp;
}

export function isUsageStale(usage: ProviderUsage, now = Date.now()): boolean {
  if (usage.status === "stale") return true;
  if (usage.status !== "connected" || usage.updatedAt == null) return false;
  return now - toMillis(usage.updatedAt) > STALE_AFTER_MS;
}

export function primaryWindow(usage: ProviderUsage): ProviderUsageWindow | null {
  if (usage.windows.length === 0) return null;
  return usage.windows.reduce((primary, window) =>
    window.usedPercent > primary.usedPercent ? window : primary
  );
}

export function formatResetLabel(resetsAt: number | null, now = Date.now()): string {
  if (resetsAt == null) return "reset time pending";
  const delta = toMillis(resetsAt) - now;
  if (delta <= 0) return "reset imminent";
  const minutes = Math.round(delta / 60_000);
  if (minutes < 1) return "resets in <1m";
  if (minutes < 60) return `resets in ${minutes}m`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) {
    const remainder = minutes % 60;
    return remainder > 0 ? `resets in ${hours}h ${remainder}m` : `resets in ${hours}h`;
  }
  const days = Math.floor(hours / 24);
  if (days < 7) return `resets in ${days}d`;
  return `resets ${new Date(toMillis(resetsAt)).toLocaleDateString(undefined, {
    month: "short",
    day: "numeric",
  })}`;
}

export function formatUpdatedAgo(updatedAt: number | null, now = Date.now()): string {
  if (updatedAt == null) return "never updated";
  const delta = Math.max(0, now - toMillis(updatedAt));
  const minutes = Math.floor(delta / 60_000);
  if (minutes < 1) return "updated just now";
  if (minutes < 60) return `updated ${minutes}m ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `updated ${hours}h ago`;
  return `updated ${Math.floor(hours / 24)}d ago`;
}

export function usageStatusLabel(usage: ProviderUsage, stale: boolean): string {
  if (stale) return "Stale data";
  switch (usage.status) {
    case "connected":
      return "Connected";
    case "loading":
      return "Checking usage";
    case "error":
      return "Usage error";
    default:
      return "Not connected";
  }
}
