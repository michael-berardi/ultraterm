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
  MemorySnapshot,
  TokenTelemetry,
  TerminalController,
  TerminalExitEvent,
  TerminalOutputEvent,
  WorkspaceSession,
} from "../types";

const MAX_PENDING_OUTPUT_BYTES = 256 * 1024;
const ACTIVITY_IDLE_DELAY_MS = 1_200;
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

export function readTerminalSlots(maxSlots: number): number[] | null {
  try {
    const stored = window.localStorage.getItem(TERMINAL_SLOTS_STORAGE_KEY);
    if (stored === null) return null;
    const parsed: unknown = JSON.parse(stored);
    if (!Array.isArray(parsed)) return null;
    return Array.from(new Set(
      parsed.filter(
        (slot): slot is number =>
          Number.isInteger(slot) && slot >= 1 && slot <= maxSlots,
      ),
    )).sort((left, right) => left - right);
  } catch {
    return null;
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
  activeSubagents: 0,
  inactiveSubagents: 0,
  parallelAgents: 0,
  trackedSessions: 0,
  updatedAt: 0,
};

interface PendingOutput {
  bytes: number;
  chunks: Uint8Array[];
}

export function useTerminalWorkspace(sessionCap: number) {
  const [sessions, setSessions] = useState<WorkspaceSession[]>([]);
  const [metrics, setMetrics] = useState<MemorySnapshot>(DEFAULT_METRICS);
  const [telemetry, setTelemetry] = useState<TokenTelemetry>(DEFAULT_TOKEN_TELEMETRY);
  const [isBooting, setIsBooting] = useState(false);
  const [isAddingPane, setIsAddingPane] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const controllers = useRef(new Map<string, TerminalController>());
  const pendingOutput = useRef(new Map<string, PendingOutput>());
  const activityTimers = useRef(new Map<string, number>());
  const bootstrapped = useRef(false);
  const addPaneInFlight = useRef(false);
  const listenersReady = useRef<Promise<void>>(Promise.resolve());
  const desktopRuntime = isTauri();
  const forgetSession = useCallback((id: string): void => {
    controllers.current.delete(id);
    pendingOutput.current.delete(id);
    const activityTimer = activityTimers.current.get(id);
    if (activityTimer) window.clearTimeout(activityTimer);
    activityTimers.current.delete(id);
  }, []);
  const clearRuntimeState = useCallback((): void => {
    controllers.current.clear();
    pendingOutput.current.clear();
    activityTimers.current.forEach((timer) => window.clearTimeout(timer));
    activityTimers.current.clear();
  }, []);

  const cleanupOrphanSlots = useCallback(async (intendedSlots: number[]): Promise<void> => {
    if (!desktopRuntime) return;
    try {
      await cleanupOrphanTmuxSlots(intendedSlots, listPersistentSlots, removePersistentSlot);
    } catch (error) {
      console.error("UltraTerm orphan tmux cleanup failed:", error);
    }
  }, [desktopRuntime]);

  useEffect(() => {
    if (!desktopRuntime) return;
    let active = true;
    const markWorking = (id: string) => {
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
        const bytes = base64ToBytes(payload.data);
        const controller = controllers.current.get(payload.id);

        if (controller) {
          controller.write(bytes);
          return;
        }

        const pending = pendingOutput.current.get(payload.id) ?? { bytes: 0, chunks: [] };
        pending.chunks.push(bytes);
        pending.bytes += bytes.byteLength;

        while (pending.bytes > MAX_PENDING_OUTPUT_BYTES && pending.chunks.length > 1) {
          pending.bytes -= pending.chunks.shift()?.byteLength ?? 0;
        }

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
      void Promise.all(unlistenPromises).then((unlisteners) => {
        unlisteners.forEach((unlisten) => unlisten());
      });
    };
  }, [desktopRuntime, forgetSession]);

  useEffect(() => {
    if (!desktopRuntime) return;
    let active = true;

    const refreshMetrics = async () => {
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
    const interval = window.setInterval(refreshMetrics, 10_000);
    return () => {
      active = false;
      window.clearInterval(interval);
    };
  }, [desktopRuntime, sessionCap, sessions.length]);

  useEffect(() => {
    if (!desktopRuntime || !bootstrapped.current || isBooting) return;
    const slots = sessions
      .map((session) => session.slot)
      .sort((left, right) => left - right);
    window.localStorage.setItem(TERMINAL_SLOTS_STORAGE_KEY, JSON.stringify(slots));
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

  const launchSlot = useCallback(async (slot: number) => {
    if (!desktopRuntime) {
      throw new Error("Terminal sessions require the UltraTerm desktop runtime.");
    }
    const info = await createSession({
      slot,
      cols: 80,
      rows: 24,
      launchOmp: true,
    });
    setSessions((current) => [
      ...current.filter((session) => session.id !== info.id && session.slot !== info.slot),
      { ...info, status: "live" as const, activity: "idle" as const },
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
          status: "live" as const,
          activity: "idle" as const,
        })));
      }

      const occupiedSlots = new Set(existing.map((session) => session.slot));
      const savedSlots = readTerminalSlots(sessionCap);
      const persistentSlots = await listPersistentSlots();
      const restorablePersistentSlots = persistentSlots.filter((slot) => slot <= sessionCap);
      let slotsToRestore: number[];
      if (savedSlots !== null) {
        slotsToRestore = savedSlots;
      } else if (restorablePersistentSlots.length > 0) {
        slotsToRestore = restorablePersistentSlots;
      } else {
        slotsToRestore = defaultTerminalSlots(targetCount, sessionCap);
      }

      const intendedSlots = Array.from(
        new Set([...existing.map((session) => session.slot), ...slotsToRestore]),
      ).sort((left, right) => left - right);
      await cleanupOrphanSlots(intendedSlots);

      for (const slot of slotsToRestore) {
        if (!occupiedSlots.has(slot)) await launchSlot(slot);
      }
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error));
    } finally {
      setIsBooting(false);
    }
  }, [desktopRuntime, cleanupOrphanSlots, launchSlot, sessionCap]);

  const addPane = useCallback(async () => {
    if (addPaneInFlight.current) return;
    addPaneInFlight.current = true;
    setIsAddingPane(true);
    setNotice(null);

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

      await launchSlot(nextSlot);
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error));
    } finally {
      addPaneInFlight.current = false;
      setIsAddingPane(false);
    }
  }, [launchSlot, metrics.maxSessions, sessionCap, sessions]);

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
      await reconnectTerminalSlot(id, session.slot, detachSession, forgetSession, launchSlot);
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
          if (!occupied.has(slot)) await launchSlot(slot);
        }
      }
      const intendedSlots = Array.from({ length: targetCount }, (_, index) => index + 1);
      await cleanupOrphanSlots(intendedSlots);
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error));
    }
  }, [cleanupOrphanSlots, forgetSession, launchSlot, sessions]);

  const restartAll = useCallback(async () => {
    const slots = sessions.map((session) => session.slot).sort((left, right) => left - right);
    setIsBooting(true);
    setNotice(null);

    try {
      await detachAllSessions();
      clearRuntimeState();
      setSessions([]);
      for (const slot of slots) await launchSlot(slot);
      await cleanupOrphanSlots(slots);
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
    setNotice(null);
    const session = sessions.find((candidate) => candidate.id === id);
    if (!session || session.status !== "live") {
      setNotice("That terminal is not connected.");
      return;
    }
    try {
      const encoded = bytesToBase64(TEXT_ENCODER.encode(`${command}\r`));
      await writeToSession(id, encoded);
      controllers.current.get(id)?.focus();
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error));
    }
  }, [sessions]);
  const dismissNotice = useCallback(() => setNotice(null), []);

  return {
    sessions,
    metrics,
    telemetry,
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
    dismissNotice,
  };
}
