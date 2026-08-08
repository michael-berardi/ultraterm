import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { isTauri } from "@tauri-apps/api/core";
import {
  base64ToBytes,
  bytesToBase64,
  closeAllSessions,
  closeSession,
  createSession,
  detachAllSessions,
  detachSession,
  listPersistentSlots,
  listSessions,
  removePersistentSlot,
  systemMetrics,
  tokenTelemetry as fetchTokenTelemetry,
  writeToSession,
} from "../lib/terminalApi";
import type {
  LaunchProfileId,
  MemorySnapshot,
  TokenTelemetry,
  TerminalController,
  TerminalExitEvent,
  TerminalOutputEvent,
  WorkspaceSession,
} from "../types";
import { isLaunchProfileId, launchProfileFromOmpProfile } from "../types";
import {
  isResizeActivitySuppressed,
  resizeActivitySuppressionDeadline,
} from "../lib/terminalActivity";

const MAX_PENDING_OUTPUT_BYTES = 256 * 1024;
const ACTIVITY_IDLE_DELAY_MS = 1_200;
const METRICS_POLL_INTERVAL_MS = 5_000;
const TELEMETRY_OUTPUT_DEBOUNCE_MS = 800;
const TELEMETRY_MIN_FETCH_GAP_MS = 1_500;
const DEFAULT_METRICS: MemorySnapshot = {
  appMemoryMib: 0,
  terminalMemoryMib: 0,
  sessionCount: 0,
  maxSessions: 8,
};
const EMPTY_TOKEN_COUNTS = {
  input: 0,
  output: 0,
  cacheRead: 0,
  cacheWrite: 0,
  total: 0,
};
const TEXT_ENCODER = new TextEncoder();
const TERMINAL_SLOTS_STORAGE_KEY = "ultraterm.open-terminal-slots";
const LAUNCH_PROFILE_STORAGE_KEY = "ultraterm.launch-profile";

export interface TerminalLaunchRow {
  slot: number;
  launchProfile: LaunchProfileId;
}

export async function cleanupOrphanTmuxSlots(
  intendedSlots: number[],
  listTmuxSlots: () => Promise<number[]>,
  removeTmuxSlot: (slot: number) => Promise<void>,
): Promise<void> {
  const tmuxSlots = await listTmuxSlots();
  const intendedSet = new Set(intendedSlots);
  const orphans = tmuxSlots.filter((slot) => !intendedSet.has(slot));
  await Promise.all(orphans.map((slot) => removeTmuxSlot(slot)));
}

export function defaultTerminalSlots(targetCount: number, maxSlots: number): number[] {
  return Array.from(
    { length: Math.min(targetCount, maxSlots) },
    (_, index) => index + 1,
  );
}

export async function reconnectTerminalSlot<T>(
  id: string,
  slot: number,
  detachClient: (sessionId: string) => Promise<void>,
  forgetClient: (sessionId: string) => void,
  launch: (terminalSlot: number) => Promise<T>,
): Promise<T> {
  await detachClient(id);
  forgetClient(id);
  return launch(slot);
}

export function ompCommandInput(command: "/new" | "/exit" | "/resume"): string {
  // Clear any partially typed prompt first. Without this, clicking a command
  // button can append `/new` to pending input and appear to do nothing.
  return `\u0015${command}\r`;
}

/**
 * Reads persisted terminal launch rows. The legacy format stored a bare array of
 * slot numbers; those entries migrate in place to the `default` launch profile.
 */
export function readTerminalLaunchRows(maxSlots: number): TerminalLaunchRow[] | null {
  try {
    const stored = window.localStorage.getItem(TERMINAL_SLOTS_STORAGE_KEY);
    if (stored === null) return null;
    const parsed: unknown = JSON.parse(stored);
    if (!Array.isArray(parsed)) return null;
    const rows = new Map<number, LaunchProfileId>();
    for (const entry of parsed) {
      const record = typeof entry === "object" && entry !== null
        ? entry as { slot?: unknown; launchProfile?: unknown }
        : null;
      const slot = record ? record.slot : entry;
      if (
        typeof slot !== "number"
        || !Number.isInteger(slot)
        || slot < 1
        || slot > maxSlots
      ) continue;
      rows.set(
        slot,
        record && isLaunchProfileId(record.launchProfile) ? record.launchProfile : "default",
      );
    }
    return Array.from(rows, ([slot, launchProfile]) => ({ slot, launchProfile }))
      .sort((left, right) => left.slot - right.slot);
  } catch {
    return null;
  }
}

export function readLastLaunchProfile(): LaunchProfileId {
  try {
    const stored = window.localStorage.getItem(LAUNCH_PROFILE_STORAGE_KEY);
    return isLaunchProfileId(stored) ? stored : "default";
  } catch {
    return "default";
  }
}

const DEFAULT_TOKEN_TELEMETRY: TokenTelemetry = {
  terminals: [1, 2, 3].map((slot) => ({
    slot,
    sessionId: null,
    model: null,
    usage: { ...EMPTY_TOKEN_COUNTS },
    activeSubagents: 0,
    inactiveSubagents: 0,
  })),
  past24Hours: { ...EMPTY_TOKEN_COUNTS },
  past7Days: { ...EMPTY_TOKEN_COUNTS },
  allTime: { ...EMPTY_TOKEN_COUNTS },
  today: { ...EMPTY_TOKEN_COUNTS },
  history: [],
  activeSubagents: 0,
  inactiveSubagents: 0,
  parallelAgents: 0,
  trackedSessions: 0,
  updatedAt: 0,
};

export interface PendingOutput {
  bytes: number;
  chunks: Uint8Array[];
}

export function appendCappedOutput(
  buffer: PendingOutput,
  chunk: Uint8Array,
  maxBytes = MAX_PENDING_OUTPUT_BYTES,
): void {
  if (maxBytes <= 0) {
    buffer.bytes = 0;
    buffer.chunks = [];
    return;
  }

  if (chunk.byteLength >= maxBytes) {
    buffer.bytes = maxBytes;
    buffer.chunks = [chunk.slice(chunk.byteLength - maxBytes)];
    return;
  }

  buffer.chunks.push(chunk);
  buffer.bytes += chunk.byteLength;

  while (buffer.bytes > maxBytes && buffer.chunks.length > 0) {
    const overflow = buffer.bytes - maxBytes;
    const oldest = buffer.chunks[0];
    if (oldest.byteLength <= overflow) {
      buffer.chunks.shift();
      buffer.bytes -= oldest.byteLength;
      continue;
    }
    buffer.chunks[0] = oldest.slice(overflow);
    buffer.bytes -= overflow;
  }
}

export function useTerminalWorkspace(sessionCap: number) {
  const [sessions, setSessions] = useState<WorkspaceSession[]>([]);
  const [metrics, setMetrics] = useState<MemorySnapshot>(DEFAULT_METRICS);
  const [telemetry, setTelemetry] = useState<TokenTelemetry>(DEFAULT_TOKEN_TELEMETRY);
  const [launchProfile, setLaunchProfile] = useState<LaunchProfileId>(readLastLaunchProfile);
  const [isBooting, setIsBooting] = useState(false);
  const [isAddingPane, setIsAddingPane] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const controllers = useRef(new Map<string, TerminalController>());
  const pendingOutput = useRef(new Map<string, PendingOutput>());
  // Output is batched per session and flushed once per animation frame, so a
  // busy pane issues a single IPC-decode-and-write per frame instead of one
  // per PTY read. This keeps echo latency smooth while typing.
  const writeBuffers = useRef(new Map<string, PendingOutput & { frame: number | null }>());
  const activityTimers = useRef(new Map<string, number>());
  const resizeActivitySuppression = useRef(new Map<string, number>());
  const telemetryFetch = useRef<{ timer: number | null; lastFetch: number }>({
    timer: null,
    lastFetch: 0,
  });
  const refreshMetricsRef = useRef<() => void>(() => undefined);
  const bootstrapped = useRef(false);
  const addPaneInFlight = useRef(false);
  const listenersReady = useRef<Promise<void>>(Promise.resolve());
  const desktopRuntime = isTauri();
  const forgetSession = useCallback((id: string): void => {
    controllers.current.delete(id);
    pendingOutput.current.delete(id);
    const writeBuffer = writeBuffers.current.get(id);
    if (writeBuffer?.frame !== null && writeBuffer?.frame !== undefined) {
      window.cancelAnimationFrame(writeBuffer.frame);
    }
    writeBuffers.current.delete(id);
    const activityTimer = activityTimers.current.get(id);
    if (activityTimer) window.clearTimeout(activityTimer);
    activityTimers.current.delete(id);
    resizeActivitySuppression.current.delete(id);
  }, []);
  const clearRuntimeState = useCallback((): void => {
    controllers.current.clear();
    pendingOutput.current.clear();
    writeBuffers.current.forEach((buffer) => {
      if (buffer.frame !== null) window.cancelAnimationFrame(buffer.frame);
    });
    writeBuffers.current.clear();
    activityTimers.current.forEach((timer) => window.clearTimeout(timer));
    activityTimers.current.clear();
    resizeActivitySuppression.current.clear();
  }, []);

  const suppressActivityForResize = useCallback((id: string): void => {
    resizeActivitySuppression.current.set(
      id,
      resizeActivitySuppressionDeadline(Date.now()),
    );
  }, []);

  const cleanupOrphanSlots = useCallback(async (intendedSlots: number[]): Promise<void> => {
    if (!desktopRuntime) return;
    try {
      const listTmuxSlots = async () => (await listPersistentSlots()).map((info) => info.slot);
      await cleanupOrphanTmuxSlots(intendedSlots, listTmuxSlots, removePersistentSlot);
    } catch (error) {
      console.error("UltraTerm orphan tmux cleanup failed:", error);
    }
  }, [desktopRuntime]);

  useEffect(() => {
    if (!desktopRuntime) return;
    let active = true;
    const markWorking = (id: string) => {
      const suppressedUntil = resizeActivitySuppression.current.get(id);
      if (isResizeActivitySuppressed(suppressedUntil, Date.now())) return;
      if (suppressedUntil !== undefined) resizeActivitySuppression.current.delete(id);

      const previousTimer = activityTimers.current.get(id);
      if (previousTimer) window.clearTimeout(previousTimer);

      setSessions((current) => {
        let changed = false;
        const next = current.map((session) => {
          if (session.id !== id || session.activity === "working") return session;
          changed = true;
          return { ...session, activity: "working" as const };
        });
        return changed ? next : current;
      });

      const timer = window.setTimeout(() => {
        activityTimers.current.delete(id);
        setSessions((current) =>
          current.map((session) =>
            session.id === id ? { ...session, activity: "idle" as const } : session,
          ),
        );
      }, ACTIVITY_IDLE_DELAY_MS);
      activityTimers.current.set(id, timer);
    };
    const unlistenPromises = [
      listen<TerminalOutputEvent>("terminal-output", ({ payload }) => {
        if (!active) return;
        markWorking(payload.id);
        const tracker = telemetryFetch.current;
        if (tracker.timer === null && Date.now() - tracker.lastFetch > TELEMETRY_MIN_FETCH_GAP_MS) {
          tracker.timer = window.setTimeout(() => {
            tracker.timer = null;
            refreshMetricsRef.current();
          }, TELEMETRY_OUTPUT_DEBOUNCE_MS);
        }
        const bytes = base64ToBytes(payload.data);
        const controller = controllers.current.get(payload.id);

        if (controller) {
          let buffer = writeBuffers.current.get(payload.id);
          if (!buffer) {
            buffer = { bytes: 0, chunks: [], frame: null };
            writeBuffers.current.set(payload.id, buffer);
          }
          appendCappedOutput(buffer, bytes);
          if (buffer.frame === null) {
            buffer.frame = window.requestAnimationFrame(() => {
              writeBuffers.current.delete(payload.id);
              const target = controllers.current.get(payload.id);
              if (!target) {
                // Controller vanished between schedule and flush: keep the
                // bytes in pendingOutput so a re-registered pane still gets them.
                const pending = pendingOutput.current.get(payload.id) ?? { bytes: 0, chunks: [] };
                for (const chunk of buffer.chunks) {
                  appendCappedOutput(pending, chunk);
                }
                pendingOutput.current.set(payload.id, pending);
                return;
              }
              const merged = new Uint8Array(buffer.bytes);
              let offset = 0;
              for (const chunk of buffer.chunks) {
                merged.set(chunk, offset);
                offset += chunk.byteLength;
              }
              target.write(merged);
            });
          }
          return;
        }

        const pending = pendingOutput.current.get(payload.id) ?? { bytes: 0, chunks: [] };
        appendCappedOutput(pending, bytes);
        pendingOutput.current.set(payload.id, pending);
      }),
      listen<TerminalExitEvent>("terminal-exit", ({ payload }) => {
        if (!active) return;
        forgetSession(payload.id);
        setSessions((current) =>
          current.map((session) =>
            session.id === payload.id
              ? { ...session, status: "exited", activity: "idle" as const }
              : session,
          ),
        );
      }),
    ];
    listenersReady.current = Promise.all(unlistenPromises).then(() => undefined);

    return () => {
      active = false;
      activityTimers.current.forEach((timer) => window.clearTimeout(timer));
      activityTimers.current.clear();
      resizeActivitySuppression.current.clear();
      writeBuffers.current.forEach((buffer) => {
        if (buffer.frame !== null) window.cancelAnimationFrame(buffer.frame);
      });
      writeBuffers.current.clear();
      if (telemetryFetch.current.timer !== null) {
        window.clearTimeout(telemetryFetch.current.timer);
        telemetryFetch.current.timer = null;
      }
      void Promise.all(unlistenPromises).then((unlisteners) => {
        unlisteners.forEach((unlisten) => unlisten());
      });
    };
  }, [desktopRuntime, forgetSession]);

  useEffect(() => {
    if (!desktopRuntime) return;
    let active = true;

    const refreshMetrics = async () => {
      telemetryFetch.current.lastFetch = Date.now();
      const [memoryResult, tokenResult] = await Promise.allSettled([
        systemMetrics(),
        fetchTokenTelemetry(),
      ]);
      if (!active) return;
      if (memoryResult.status === "fulfilled") {
        setMetrics({
          ...memoryResult.value,
          maxSessions: Math.min(memoryResult.value.maxSessions, sessionCap),
        });
      } else {
        setMetrics((current) => ({
          ...current,
          sessionCount: sessions.length,
          maxSessions: Math.min(current.maxSessions, sessionCap),
        }));
      }
      if (tokenResult.status === "fulfilled") setTelemetry(tokenResult.value);
    };

    void refreshMetrics();
    refreshMetricsRef.current = () => void refreshMetrics();
    const interval = window.setInterval(refreshMetrics, METRICS_POLL_INTERVAL_MS);
    return () => {
      active = false;
      refreshMetricsRef.current = () => undefined;
      window.clearInterval(interval);
    };
  }, [desktopRuntime, sessionCap, sessions.length]);

  useEffect(() => {
    if (!desktopRuntime || !bootstrapped.current || isBooting) return;
    const rows: TerminalLaunchRow[] = sessions
      .map((session) => ({ slot: session.slot, launchProfile: session.launchProfile }))
      .sort((left, right) => left.slot - right.slot);
    window.localStorage.setItem(TERMINAL_SLOTS_STORAGE_KEY, JSON.stringify(rows));
  }, [desktopRuntime, isBooting, sessions]);

  const registerController = useCallback((id: string, controller: TerminalController | null) => {
    if (!controller) {
      controllers.current.delete(id);
      return;
    }

    controllers.current.set(id, controller);
    const pending = pendingOutput.current.get(id);
    if (pending) {
      pending.chunks.forEach((chunk) => controller.write(chunk));
      pendingOutput.current.delete(id);
    }
  }, []);

  const launchSlot = useCallback(async (slot: number, profile: LaunchProfileId) => {
    if (!desktopRuntime) {
      throw new Error("Terminal sessions require the UltraTerm desktop runtime.");
    }
    const info = await createSession({
      slot,
      cols: 80,
      rows: 24,
      launchOmp: true,
      launchProfile: profile,
    });
    setSessions((current) => [
      ...current.filter((session) => session.id !== info.id && session.slot !== info.slot),
      {
        ...info,
        launchProfile: info.launchProfile ?? profile,
        status: "live" as const,
        activity: "idle" as const,
      },
    ].sort((left, right) => left.slot - right.slot));
    return info;
  }, [desktopRuntime]);

  const reconcileSessions = useCallback(async () => {
    const existing = await listSessions();
    const remainingIds = new Set(existing.map((session) => session.id));
    for (const id of controllers.current.keys()) {
      if (!remainingIds.has(id)) controllers.current.delete(id);
    }
    for (const id of pendingOutput.current.keys()) {
      if (!remainingIds.has(id)) pendingOutput.current.delete(id);
    }
    for (const [id, timer] of activityTimers.current) {
      if (remainingIds.has(id)) continue;
      window.clearTimeout(timer);
      activityTimers.current.delete(id);
    }
    setSessions(existing.map((session) => ({
      ...session,
      launchProfile: session.launchProfile ?? "default",
      status: "live" as const,
      activity: "idle" as const,
    })));
  }, []);

  const bootstrap = useCallback(async (targetCount: number) => {
    if (!desktopRuntime) return;
    if (bootstrapped.current) return;
    bootstrapped.current = true;
    setIsBooting(true);
    setNotice(null);

    try {
      await listenersReady.current;
      const existing = await listSessions();
      if (existing.length > 0) {
        setSessions(existing.map((session) => ({
          ...session,
          launchProfile: session.launchProfile ?? "default",
          status: "live" as const,
          activity: "idle" as const,
        })));
      }

      const occupiedSlots = new Set(existing.map((session) => session.slot));
      const savedRows = readTerminalLaunchRows(sessionCap);
      const persistentSlots = await listPersistentSlots();
      const restorablePersistentSlots = persistentSlots.filter((info) => info.slot <= sessionCap);
      let rowsToRestore: TerminalLaunchRow[];
      if (restorablePersistentSlots.length > 0) {
        // Live tmux sessions are the source of truth: restore exactly those
        // slots with the launch profile each session recorded, so omp-safe
        // reattaches instead of rejecting a mismatched signature.
        rowsToRestore = restorablePersistentSlots.map((info) => ({
          slot: info.slot,
          launchProfile: launchProfileFromOmpProfile(info.profile),
        }));
      } else if (savedRows !== null) {
        rowsToRestore = savedRows;
      } else {
        rowsToRestore = defaultTerminalSlots(targetCount, sessionCap).map((slot) => ({
          slot,
          launchProfile,
        }));
      }

      const intendedSlots = Array.from(
        new Set([...existing.map((session) => session.slot), ...rowsToRestore.map((row) => row.slot)]),
      ).sort((left, right) => left - right);
      await cleanupOrphanSlots(intendedSlots);

      for (const row of rowsToRestore) {
        if (!occupiedSlots.has(row.slot)) await launchSlot(row.slot, row.launchProfile);
      }
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error));
    } finally {
      setIsBooting(false);
    }
  }, [desktopRuntime, cleanupOrphanSlots, launchProfile, launchSlot, sessionCap]);

  const addPane = useCallback(async (profile?: LaunchProfileId) => {
    if (addPaneInFlight.current) return;
    addPaneInFlight.current = true;
    setIsAddingPane(true);
    setNotice(null);
    const resolvedProfile = profile ?? launchProfile;
    if (profile) {
      setLaunchProfile(profile);
      try {
        window.localStorage.setItem(LAUNCH_PROFILE_STORAGE_KEY, profile);
      } catch {
        // Profile persistence is best-effort; the launch still proceeds.
      }
    }

    try {
      const maxSessions = Math.min(sessionCap, metrics.maxSessions);
      if (sessions.length >= maxSessions) {
        setNotice(`UltraTerm is capped at ${maxSessions} live terminals on this display.`);
        return;
      }
      const occupied = new Set(sessions.map((session) => session.slot));
      const nextSlot = Array.from({ length: maxSessions }, (_, index) => index + 1)
        .find((slot) => !occupied.has(slot));

      if (!nextSlot) {
        setNotice(`UltraTerm is capped at ${maxSessions} live terminals on this display.`);
        return;
      }

      await launchSlot(nextSlot, resolvedProfile);
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error));
    } finally {
      addPaneInFlight.current = false;
      setIsAddingPane(false);
    }
  }, [launchProfile, launchSlot, metrics.maxSessions, sessionCap, sessions]);

  const removePane = useCallback(async (id: string) => {
    const session = sessions.find((candidate) => candidate.id === id);
    if (!session) return;
    setNotice(null);
    try {
      if (session.status === "live") await closeSession(id);
      forgetSession(id);
      setSessions((current) => current.filter((session) => session.id !== id));
      const remainingSlots = sessions
        .filter((candidate) => candidate.id !== id)
        .map((candidate) => candidate.slot);
      await cleanupOrphanSlots(remainingSlots);
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error));
    }
  }, [cleanupOrphanSlots, forgetSession, sessions]);

  const closePanes = useCallback(async (ids: string[]) => {
    const requested = new Set(ids);
    const targets = sessions.filter((session) => requested.has(session.id));
    await Promise.all(targets.map((session) => removePane(session.id)));
    const remainingSlots = sessions
      .filter((session) => !requested.has(session.id))
      .map((session) => session.slot);
    await cleanupOrphanSlots(remainingSlots);
  }, [cleanupOrphanSlots, removePane, sessions]);

  const restartPane = useCallback(async (id: string) => {
    const session = sessions.find((candidate) => candidate.id === id);
    if (!session) return;

    setNotice(null);
    try {
      await reconnectTerminalSlot(
        id,
        session.slot,
        detachSession,
        forgetSession,
        (slot) => launchSlot(slot, session.launchProfile),
      );
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error));
    }
  }, [forgetSession, launchSlot, sessions]);

  const rebalance = useCallback(async (targetCount: number) => {
    setNotice(null);
    const ordered = [...sessions].sort((left, right) => left.slot - right.slot);

    try {
      if (ordered.length > targetCount) {
        const surplus = ordered.slice(targetCount);
        await Promise.all(surplus.map((session) => closeSession(session.id)));
        const removedIds = new Set(surplus.map((session) => session.id));
        removedIds.forEach((id) => {
          forgetSession(id);
        });
        setSessions((current) => current.filter((session) => !removedIds.has(session.id)));
      } else {
        const occupied = new Set(ordered.map((session) => session.slot));
        for (let slot = 1; slot <= targetCount; slot += 1) {
          if (!occupied.has(slot)) await launchSlot(slot, launchProfile);
        }
      }
      const intendedSlots = Array.from({ length: targetCount }, (_, index) => index + 1);
      await cleanupOrphanSlots(intendedSlots);
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error));
    }
  }, [cleanupOrphanSlots, forgetSession, launchProfile, launchSlot, sessions]);

  const restartAll = useCallback(async () => {
    const rows: TerminalLaunchRow[] = sessions
      .map((session) => ({ slot: session.slot, launchProfile: session.launchProfile }))
      .sort((left, right) => left.slot - right.slot);
    setIsBooting(true);
    setNotice(null);

    try {
      await detachAllSessions();
      clearRuntimeState();
      setSessions([]);
      for (const row of rows) await launchSlot(row.slot, row.launchProfile);
      await cleanupOrphanSlots(rows.map((row) => row.slot));
    } catch (error) {
      await reconcileSessions().catch((reconcileError) => {
        console.error("UltraTerm could not reconcile terminal clients", reconcileError);
      });
      setNotice(error instanceof Error ? error.message : String(error));
    } finally {
      setIsBooting(false);
    }
  }, [cleanupOrphanSlots, clearRuntimeState, launchSlot, reconcileSessions, sessions]);

  const disconnectAll = useCallback(async () => {
    setNotice(null);
    try {
      await closeAllSessions();
      clearRuntimeState();
      setSessions([]);
      await cleanupOrphanSlots([]);
    } catch (error) {
      await reconcileSessions().catch((reconcileError) => {
        console.error("UltraTerm could not reconcile terminal clients", reconcileError);
      });
      setNotice(error instanceof Error ? error.message : String(error));
    }
  }, [cleanupOrphanSlots, clearRuntimeState, reconcileSessions]);

  const focusPane = useCallback((id: string) => {
    controllers.current.get(id)?.focus();
  }, []);
  const scrollPane = useCallback((id: string, lines: number) => {
    controllers.current.get(id)?.scrollLines(lines);
  }, []);
  const scrollPanePages = useCallback((id: string, pages: number) => {
    controllers.current.get(id)?.scrollPages(pages);
  }, []);
  const scrollPaneToBottom = useCallback((id: string) => {
    controllers.current.get(id)?.scrollToBottom();
  }, []);
  const hasPendingInput = useCallback((id: string) => (
    controllers.current.get(id)?.hasPendingInput() ?? false
  ), []);

  const sendTerminalInput = useCallback(async (id: string, data: string) => {
    setNotice(null);
    const session = sessions.find((candidate) => candidate.id === id);
    if (!session || session.status !== "live") {
      setNotice("That terminal is not connected.");
      return false;
    }
    try {
      await writeToSession(id, bytesToBase64(TEXT_ENCODER.encode(data)));
      const controller = controllers.current.get(id);
      controller?.trackInput(data);
      controller?.focus();
      return true;
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error));
      return false;
    }
  }, [sessions]);

  const sendOmpCommand = useCallback(async (id: string, command: "/new" | "/exit" | "/resume") => {
    await sendTerminalInput(id, ompCommandInput(command));
  }, [sendTerminalInput]);
  const dismissNotice = useCallback(() => setNotice(null), []);

  return {
    sessions,
    metrics,
    telemetry,
    launchProfile,
    isBooting,
    isAddingPane,
    notice,
    bootstrap,
    addPane,
    removePane,
    closePanes,
    restartPane,
    rebalance,
    restartAll,
    disconnectAll,
    registerController,
    focusPane,
    scrollPane,
    scrollPanePages,
    scrollPaneToBottom,
    hasPendingInput,
    sendTerminalInput,
    sendOmpCommand,
    suppressActivityForResize,
    dismissNotice,
  };
}
