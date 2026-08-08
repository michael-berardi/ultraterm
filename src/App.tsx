import { useCallback, useEffect, useLayoutEffect, useRef, useState, type CSSProperties, type ReactElement } from "react";
import { isTauri } from "@tauri-apps/api/core";
import { LogicalPosition, LogicalSize } from "@tauri-apps/api/dpi";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import "./App.css";
import { AmbientField } from "./components/AmbientField";
import { TerminalPane } from "./components/TerminalPane";
import { WorkspaceRail } from "./components/WorkspaceRail";
import { ControllerModal } from "./components/ControllerModal";
import { VoicePreviewModal } from "./components/VoicePreviewModal";
import { useTerminalWorkspace } from "./hooks/useTerminalWorkspace";
import { usePs4Controller } from "./hooks/usePs4Controller";
import {
  cancelVoiceInput,
  finishVoiceInput,
  startVoiceInput,
  voiceInputStatus,
} from "./lib/terminalApi";
import {
  DEFAULT_TERMINAL_PREFERENCES,
  TERMINAL_SCROLLBACK,
  type EffectMode,
  type LaunchProfileId,
  type TerminalController,
  type ThemeId,
  type TerminalPreferences,
  type VoiceInputState,
} from "./types";

const DEFAULT_TERMINAL_COUNT = 3;
const STANDARD_SESSION_CAP = 6;
const ULTRAWIDE_SESSION_CAP = 8;
const ULTRAWIDE_ASPECT_RATIO = 2.1;
const SPACE_HOLD_MILLISECONDS = 700;
const WINDOW_GEOMETRY_STORAGE_KEY = "ultraterm.window-geometry";
const WINDOW_GEOMETRY_SAVE_DELAY_MILLISECONDS = 150;
const TERMINAL_FONT_SIZE_STORAGE_KEY = "ultraterm.terminal-font-size";
const TERMINAL_CURSOR_STYLE_STORAGE_KEY = "ultraterm.terminal-cursor-style";
const TERMINAL_CURSOR_BLINK_STORAGE_KEY = "ultraterm.terminal-cursor-blink";
const EFFECT_MODE_STORAGE_KEY = "ultraterm.effects.v2";
const MINIMUM_TERMINAL_FONT_SIZE = 9;
const PANE_EXIT_DURATION_MILLISECONDS = 180;
const PANE_REFLOW_DURATION_MILLISECONDS = 220;
const PANE_REFLOW_REVEAL_SCALE = 0.985;
const PANE_MOTION_EASING = "cubic-bezier(0.22, 1, 0.36, 1)";
const MAXIMUM_TERMINAL_FONT_SIZE = 18;
const BOOT_SPLASH_MIN_MS = 650;
const BOOT_SPLASH_MAX_MS = 5000;

function readTerminalPreferences(): TerminalPreferences {
  try {
    const storedFontSize = Number(localStorage.getItem(TERMINAL_FONT_SIZE_STORAGE_KEY));
    const storedCursorStyle = localStorage.getItem(TERMINAL_CURSOR_STYLE_STORAGE_KEY);
    const storedCursorBlink = localStorage.getItem(TERMINAL_CURSOR_BLINK_STORAGE_KEY);
    const fontSize = Number.isInteger(storedFontSize)
      && storedFontSize >= MINIMUM_TERMINAL_FONT_SIZE
      && storedFontSize <= MAXIMUM_TERMINAL_FONT_SIZE
      ? storedFontSize
      : DEFAULT_TERMINAL_PREFERENCES.fontSize;
    const cursorStyle = storedCursorStyle === "bar"
      || storedCursorStyle === "block"
      || storedCursorStyle === "underline"
      ? storedCursorStyle
      : DEFAULT_TERMINAL_PREFERENCES.cursorStyle;
    const cursorBlink = storedCursorBlink === "false"
      ? false
      : storedCursorBlink === "true"
        ? true
        : DEFAULT_TERMINAL_PREFERENCES.cursorBlink;

    return { fontSize, cursorStyle, cursorBlink };
  } catch {
    return { ...DEFAULT_TERMINAL_PREFERENCES };
  }
}

interface SavedWindowGeometry {
  x: number;
  y: number;
  width: number;
  height: number;
  maximized: boolean;
}

function readSavedWindowGeometry(): SavedWindowGeometry | null {
  try {
    const parsed = JSON.parse(localStorage.getItem(WINDOW_GEOMETRY_STORAGE_KEY) ?? "null") as
      | Partial<SavedWindowGeometry>
      | null;
    const { x, y, width, height, maximized } = parsed ?? {};
    if (
      typeof x !== "number"
      || !Number.isFinite(x)
      || typeof y !== "number"
      || !Number.isFinite(y)
      || typeof width !== "number"
      || !Number.isFinite(width)
      || width <= 0
      || typeof height !== "number"
      || !Number.isFinite(height)
      || height <= 0
      || typeof maximized !== "boolean"
    ) {
      return null;
    }
    return { x, y, width, height, maximized };
  } catch {
    return null;
  }
}

function useWindowGeometryPersistence(): void {
  useEffect(() => {
    if (!isTauri()) return;

    const appWindow = getCurrentWindow();
    let disposed = false;
    let saveTimer: number | null = null;
    let unlistenMoved: (() => void) | null = null;
    let unlistenResized: (() => void) | null = null;

    const saveGeometry = async () => {
      if (disposed) return;
      const [position, size, scaleFactor, maximized] = await Promise.all([
        appWindow.outerPosition(),
        appWindow.outerSize(),
        appWindow.scaleFactor(),
        appWindow.isMaximized(),
      ]);
      if (disposed) return;
      const logicalPosition = position.toLogical(scaleFactor);
      const logicalSize = size.toLogical(scaleFactor);
      const geometry = {
        x: logicalPosition.x,
        y: logicalPosition.y,
        width: logicalSize.width,
        height: logicalSize.height,
        maximized,
      } satisfies SavedWindowGeometry;
      localStorage.setItem(WINDOW_GEOMETRY_STORAGE_KEY, JSON.stringify(geometry));
    };

    const queueSave = () => {
      if (saveTimer !== null) window.clearTimeout(saveTimer);
      saveTimer = window.setTimeout(() => {
        saveTimer = null;
        void saveGeometry();
      }, WINDOW_GEOMETRY_SAVE_DELAY_MILLISECONDS);
    };

    void (async () => {
      try {
        const saved = readSavedWindowGeometry();
        if (saved) {
          await appWindow.unmaximize();
          await appWindow.setSize(new LogicalSize(saved.width, saved.height));
          await appWindow.setPosition(new LogicalPosition(saved.x, saved.y));
          if (saved.maximized) await appWindow.maximize();
        }
      } catch (error) {
        console.error("Unable to restore UltraTerm window geometry", error);
      }

      if (disposed) return;
      // The window is normally visible from native creation and the inline
      // splash covers first paint. Keep this idempotent reveal for externally
      // hidden windows, but never gate it on rAF: hidden windows do not paint.
      await appWindow.show();
      await appWindow.setFocus();
      [unlistenMoved, unlistenResized] = await Promise.all([
        appWindow.onMoved(queueSave),
        appWindow.onResized(queueSave),
      ]);
    })();

    return () => {
      disposed = true;
      if (saveTimer !== null) window.clearTimeout(saveTimer);
      unlistenMoved?.();
      unlistenResized?.();
    };
  }, []);
}


function displaySessionCap(): number {
  const { width, height } = window.screen;
  return width / Math.max(height, 1) >= ULTRAWIDE_ASPECT_RATIO
    ? ULTRAWIDE_SESSION_CAP
    : STANDARD_SESSION_CAP;
}

function paneGrid(count: number): { columns: number; rows: number } {
  if (count <= 1) return { columns: 1, rows: 1 };
  if (count === 2) return { columns: 2, rows: 1 };
  if (count === 3) return { columns: 3, rows: 1 };
  if (count <= 6) return { columns: 3, rows: 2 };
  return { columns: 4, rows: 2 };
}

function usePrefersReducedMotion(): boolean {
  const [prefersReducedMotion, setPrefersReducedMotion] = useState(
    () => window.matchMedia("(prefers-reduced-motion: reduce)").matches,
  );

  useEffect(() => {
    const query = window.matchMedia("(prefers-reduced-motion: reduce)");
    const updatePreference = () => setPrefersReducedMotion(query.matches);
    updatePreference();
    query.addEventListener("change", updatePreference);
    return () => query.removeEventListener("change", updatePreference);
  }, []);

  return prefersReducedMotion;
}


type VoiceActivationSource = "keyboard" | "controller";

interface VoiceSession {
  state: VoiceInputState;
  activationSource: VoiceActivationSource | null;
  recordingId: string | null;
  destinationId: string | null;
  transcript: string;
  error: string | null;
  levels: number[];
}

const IDLE_VOICE_SESSION: VoiceSession = {
  state: "idle",
  activationSource: null,
  recordingId: null,
  destinationId: null,
  transcript: "",
  error: null,
  levels: [],
};

function sleep(milliseconds: number): Promise<void> {
  return new Promise((resolve) => window.setTimeout(resolve, milliseconds));
}

function withTimeout<T>(
  promise: Promise<T>,
  milliseconds: number,
  message: string,
  onLateResult?: (value: T) => void,
): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    let settled = false;
    const timer = window.setTimeout(() => {
      settled = true;
      reject(new Error(message));
    }, milliseconds);
    promise.then(
      (value) => {
        if (settled) {
          onLateResult?.(value);
          return;
        }
        settled = true;
        window.clearTimeout(timer);
        resolve(value);
      },
      (error) => {
        if (settled) return;
        settled = true;
        window.clearTimeout(timer);
        reject(error);
      },
    );
  });
}

function App(): ReactElement {
  useWindowGeometryPersistence();
  const [sessionCap, setSessionCap] = useState(displaySessionCap);
  const workspace = useTerminalWorkspace(sessionCap);
  const [activeId, setActiveId] = useState<string | null>(null);
  const [maximizedId, setMaximizedId] = useState<string | null>(null);
  const [selectedIds, setSelectedIds] = useState<Set<string>>(() => new Set());
  const [theme, setTheme] = useState<ThemeId>(() => {
    const stored = localStorage.getItem("ultraterm.theme");
    return stored === "aurora" || stored === "titanium" || stored === "ember" ? stored : "oled";
  });
  const [effectMode, setEffectMode] = useState<EffectMode>(() => {
    const stored = localStorage.getItem(EFFECT_MODE_STORAGE_KEY);
    return stored === "off" || stored === "ambient" || stored === "focus" || stored === "spectrum"
      ? stored
      : "focus";
  });
  const [terminalPreferences, setTerminalPreferences] = useState<TerminalPreferences>(
    readTerminalPreferences,
  );
  const [controllerOpen, setControllerOpen] = useState(false);
  const [controllerNotice, setControllerNotice] = useState<string | null>(null);
  const [voice, setVoice] = useState<VoiceSession>(IDLE_VOICE_SESSION);
  const voiceOperation = useRef(0);
  const voiceCancellation = useRef(false);
  const voiceStartInFlight = useRef(false);
  const voiceInsertion = useRef(false);
  const [voiceInserting, setVoiceInserting] = useState(false);
  const voiceRef = useRef(voice);
  voiceRef.current = voice;
  const [exitingIds, setExitingIds] = useState<Set<string>>(() => new Set());
  const prefersReducedMotion = usePrefersReducedMotion();
  const workspaceGridRef = useRef<HTMLElement>(null);
  const paneRectsRef = useRef(new Map<string, DOMRect>());
  const visiblePaneIdsRef = useRef(new Set<string>());
  const paneAnimationsRef = useRef(new Map<string, Animation>());
  const closeTimersRef = useRef(new Set<number>());
  const closingIdsRef = useRef(new Set<string>());
  const mountedRef = useRef(true);
  const sessionsRef = useRef(workspace.sessions);
  const closePanesRef = useRef(workspace.closePanes);
  sessionsRef.current = workspace.sessions;
  closePanesRef.current = workspace.closePanes;
  const paneLayoutKey = workspace.sessions.map((session) => session.id).join("\u0000");
  const layoutMaximizedId = maximizedId
    && workspace.sessions.some((session) => session.id === maximizedId)
    ? maximizedId
    : null;

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
      for (const timer of closeTimersRef.current) window.clearTimeout(timer);
      closeTimersRef.current.clear();
      for (const animation of paneAnimationsRef.current.values()) animation.cancel();
      paneAnimationsRef.current.clear();
      closingIdsRef.current.clear();
    };
  }, []);

  useLayoutEffect(() => {
    const grid = workspaceGridRef.current;
    if (!grid) return;

    for (const animation of paneAnimationsRef.current.values()) animation.cancel();
    paneAnimationsRef.current.clear();

    const paneElements = Array.from(
      grid.querySelectorAll<HTMLElement>(".terminal-pane[data-session-id]"),
    );
    const elementsById = new Map<string, HTMLElement>();
    const allPaneIds = new Set<string>();
    const currentVisibleIds = new Set<string>();
    const currentRects = new Map<string, DOMRect>();
    for (const element of paneElements) {
      const id = element.dataset.sessionId;
      if (!id) continue;
      elementsById.set(id, element);
      allPaneIds.add(id);
      const rect = element.getBoundingClientRect();
      if (rect.width <= 0 || rect.height <= 0) continue;
      currentVisibleIds.add(id);
      currentRects.set(id, rect);
    }

    const previousRects = paneRectsRef.current;
    const previousVisibleIds = visiblePaneIdsRef.current;
    const nextRects = new Map(previousRects);
    for (const id of nextRects.keys()) {
      if (!allPaneIds.has(id)) nextRects.delete(id);
    }
    for (const [id, rect] of currentRects) nextRects.set(id, rect);
    paneRectsRef.current = nextRects;
    visiblePaneIdsRef.current = currentVisibleIds;

    if (prefersReducedMotion) return;

    for (const [id, currentRect] of currentRects) {
      const previousRect = previousRects.get(id);
      const element = elementsById.get(id);
      if (!previousRect || !element || element.classList.contains("is-exiting")) continue;

      let keyframes: Keyframe[];
      if (!previousVisibleIds.has(id)) {
        keyframes = [
          {
            opacity: 0,
            transform: `scale3d(${PANE_REFLOW_REVEAL_SCALE}, ${PANE_REFLOW_REVEAL_SCALE}, 1)`,
          },
          { opacity: 1, transform: "none" },
        ];
      } else {
        const translateX = previousRect.left - currentRect.left;
        const translateY = previousRect.top - currentRect.top;
        const scaleX = previousRect.width / currentRect.width;
        const scaleY = previousRect.height / currentRect.height;
        const geometryChanged = Math.abs(translateX) > 0.5
          || Math.abs(translateY) > 0.5
          || Math.abs(scaleX - 1) > 0.001
          || Math.abs(scaleY - 1) > 0.001;
        if (!geometryChanged) continue;
        keyframes = [
          {
            transform: `translate3d(${translateX}px, ${translateY}px, 0) scale3d(${scaleX}, ${scaleY}, 1)`,
          },
          { transform: "none" },
        ];
      }

      const animation = element.animate(keyframes, {
        duration: PANE_REFLOW_DURATION_MILLISECONDS,
        easing: PANE_MOTION_EASING,
      });
      paneAnimationsRef.current.set(id, animation);
      const forgetAnimation = () => {
        if (paneAnimationsRef.current.get(id) === animation) {
          paneAnimationsRef.current.delete(id);
        }
      };
      animation.onfinish = forgetAnimation;
      animation.oncancel = forgetAnimation;
    }
  }, [layoutMaximizedId, paneLayoutKey, prefersReducedMotion]);

  useEffect(() => {
    let cleanupStarted = false;
    const cancelActiveVoice = () => {
      if (cleanupStarted) return;
      const current = voiceRef.current;
      if (
        current.recordingId
        && (current.state === "recording" || current.state === "transcribing")
      ) {
        cleanupStarted = true;
        void cancelVoiceInput(current.recordingId);
      }
    };
    window.addEventListener("beforeunload", cancelActiveVoice);
    return () => {
      window.removeEventListener("beforeunload", cancelActiveVoice);
      cancelActiveVoice();
    };
  }, []);

  useEffect(() => {
    const refreshSessionCap = () => setSessionCap(displaySessionCap());
    window.addEventListener("focus", refreshSessionCap);
    window.addEventListener("resize", refreshSessionCap);
    return () => {
      window.removeEventListener("focus", refreshSessionCap);
      window.removeEventListener("resize", refreshSessionCap);
    };
  }, []);

  const [bootStarted, setBootStarted] = useState(false);
  const [splash, setSplash] = useState<"visible" | "exiting" | "gone">("visible");
  const [mountedControllers, setMountedControllers] = useState<ReadonlySet<string>>(new Set());
  const bootStartedAt = useRef(Date.now());
  const { registerController } = workspace;

  useEffect(() => {
    setBootStarted(true);
    void workspace.bootstrap(DEFAULT_TERMINAL_COUNT);
  }, [workspace.bootstrap]);

  // TerminalPane recreates xterm whenever this callback changes. Depending on
  // the whole workspace object here causes a render/remount loop because the
  // hook returns a new wrapper object on every state update.
  const handleControllerReady = useCallback((id: string, controller: TerminalController | null) => {
    registerController(id, controller);
    setMountedControllers((current) => {
      const has = current.has(id);
      if (controller !== null && !has) return new Set(current).add(id);
      if (controller === null && has) {
        const next = new Set(current);
        next.delete(id);
        return next;
      }
      return current;
    });
  }, [registerController]);

  // The splash lifts only when the workspace is genuinely ready: bootstrap
  // finished and every restored terminal has mounted its xterm controller.
  const workspaceReady = bootStarted
    && !workspace.isBooting
    && workspace.sessions.every((session) => mountedControllers.has(session.id));

  useEffect(() => {
    if (!workspaceReady) return;
    const elapsed = Date.now() - bootStartedAt.current;
    const remaining = Math.max(0, BOOT_SPLASH_MIN_MS - elapsed);
    const timer = window.setTimeout(() => {
      setSplash((current) => (current === "visible" ? "exiting" : current));
    }, remaining);
    return () => window.clearTimeout(timer);
  }, [workspaceReady]);

  useEffect(() => {
    // Hard cap: a stuck terminal must never trap the splash.
    const timer = window.setTimeout(() => {
      setSplash((current) => (current === "visible" ? "exiting" : current));
    }, BOOT_SPLASH_MAX_MS);
    return () => window.clearTimeout(timer);
  }, []);

  useEffect(() => {
    const availableIds = new Set(workspace.sessions.map((session) => session.id));
    const fallbackId = workspace.sessions[0]?.id ?? null;

    setActiveId((current) => current && availableIds.has(current) ? current : fallbackId);
    setMaximizedId((current) => current && availableIds.has(current) ? current : null);
    setSelectedIds((current) => {
      const next = new Set([...current].filter((id) => availableIds.has(id)));
      if (next.size === 0 && fallbackId) next.add(fallbackId);
      if (next.size === current.size && [...next].every((id) => current.has(id))) {
        return current;
      }
      return next;
    });
  }, [workspace.sessions]);


  useEffect(() => {
    localStorage.setItem(EFFECT_MODE_STORAGE_KEY, effectMode);
  }, [effectMode]);

  useEffect(() => {
    localStorage.setItem("ultraterm.theme", theme);
  }, [theme]);

  useEffect(() => {
    try {
      localStorage.setItem(TERMINAL_FONT_SIZE_STORAGE_KEY, String(terminalPreferences.fontSize));
      localStorage.setItem(TERMINAL_CURSOR_STYLE_STORAGE_KEY, terminalPreferences.cursorStyle);
      localStorage.setItem(TERMINAL_CURSOR_BLINK_STORAGE_KEY, String(terminalPreferences.cursorBlink));
    } catch {
      // Settings remain live for this session when persistent storage is unavailable.
    }
  }, [terminalPreferences]);

  const selectedTerminalIds = useCallback((): string[] => {
    const targets: string[] = [];
    for (const session of workspace.sessions) {
      if (selectedIds.has(session.id)) targets.push(session.id);
    }
    if (targets.length === 0 && activeId) targets.push(activeId);
    return targets;
  }, [activeId, selectedIds, workspace.sessions]);

  const requestCloseTerminals = useCallback((requestedIds: readonly string[]): void => {
    const sessionsById = new Map(sessionsRef.current.map((session) => [session.id, session]));
    const targetIds: string[] = [];
    const liveTargetIds: string[] = [];
    const seenIds = new Set<string>();
    for (const id of requestedIds) {
      if (seenIds.has(id) || closingIdsRef.current.has(id)) continue;
      seenIds.add(id);
      const session = sessionsById.get(id);
      if (!session) continue;
      closingIdsRef.current.add(id);
      targetIds.push(id);
      if (session.status === "live") liveTargetIds.push(id);
    }
    if (targetIds.length === 0) return;

    for (const id of liveTargetIds) {
      paneAnimationsRef.current.get(id)?.cancel();
      paneAnimationsRef.current.delete(id);
    }
    if (liveTargetIds.length > 0) {
      setExitingIds((current) => {
        const next = new Set(current);
        for (const id of liveTargetIds) next.add(id);
        return next;
      });
    }

    let timer: number | null = null;
    const invokeClose = () => {
      if (timer !== null) closeTimersRef.current.delete(timer);
      void (async () => {
        try {
          await closePanesRef.current(targetIds);
        } catch (error) {
          console.error("UltraTerm could not close terminal clients", error);
        } finally {
          for (const id of targetIds) closingIdsRef.current.delete(id);
          if (!mountedRef.current) return;
          setExitingIds((current) => {
            const next = new Set(current);
            let changed = false;
            for (const id of liveTargetIds) changed = next.delete(id) || changed;
            return changed ? next : current;
          });
        }
      })();
    };

    if (liveTargetIds.length > 0 && !prefersReducedMotion) {
      timer = window.setTimeout(invokeClose, PANE_EXIT_DURATION_MILLISECONDS);
      closeTimersRef.current.add(timer);
    } else {
      invokeClose();
    }
  }, [prefersReducedMotion]);

  const closeSelectedTerminals = useCallback((): void => {
    requestCloseTerminals(selectedTerminalIds());
  }, [requestCloseTerminals, selectedTerminalIds]);

  const addTerminal = useCallback((profile?: LaunchProfileId): void => {
    if (
      workspace.isBooting
      || workspace.isAddingPane
      || workspace.sessions.length >= workspace.metrics.maxSessions
    ) return;
    void workspace.addPane(profile);
  }, [
    workspace.addPane,
    workspace.isAddingPane,
    workspace.isBooting,
    workspace.metrics.maxSessions,
    workspace.sessions.length,
  ]);

  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      if (controllerOpen || voice.state !== "idle") return;

      if (
        event.metaKey
        && !event.altKey
        && !event.ctrlKey
        && !event.shiftKey
        && (event.code === "Backspace" || event.code === "Delete")
      ) {
        if (event.repeat || event.isComposing) return;
        const target = event.target instanceof Element ? event.target : null;
        const insideTerminal = Boolean(target?.closest(".terminal-pane"));
        if (
          !insideTerminal
          && target?.closest("input, textarea, select, [contenteditable='true'], [role='dialog']")
        ) return;

        const targets = selectedTerminalIds();
        if (targets.length === 0) return;
        event.preventDefault();
        event.stopImmediatePropagation();
        requestCloseTerminals(targets);
        return;
      }

      if (
        event.metaKey
        && !event.altKey
        && !event.ctrlKey
        && !event.shiftKey
        && event.code === "KeyT"
      ) {
        event.preventDefault();
        if (!event.repeat && !event.isComposing) addTerminal();
        return;
      }

      if (event.metaKey && /^[1-9]$/.test(event.key)) {
        const session = workspace.sessions[Number(event.key) - 1];
        if (session) {
          event.preventDefault();
          setActiveId(session.id);
          setSelectedIds(new Set([session.id]));
          if (maximizedId) setMaximizedId(session.id);
          workspace.focusPane(session.id);
        }
        return;
      }

      if (event.key === "Escape" && maximizedId) setMaximizedId(null);
    };

    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [
    addTerminal,
    controllerOpen,
    maximizedId,
    requestCloseTerminals,
    selectedTerminalIds,
    voice.state,
    workspace.focusPane,
    workspace.sessions,
  ]);

  useEffect(() => {
    const unlistenPromise = listen("tauri://focus", () => {
      if (voice.state === "idle" && !controllerOpen && activeId) {
        workspace.focusPane(activeId);
      }
    });
    return () => {
      void unlistenPromise.then((unlisten) => unlisten());
    };
  }, [activeId, controllerOpen, voice.state, workspace.focusPane]);

  const selectTerminal = useCallback((id: string, extendSelection = false): void => {
    setSelectedIds((current) => {
      if (!extendSelection) return new Set([id]);
      const next = new Set(current);
      next.add(id);
      return next;
    });
    setActiveId(id);
    if (maximizedId) setMaximizedId(id);
    workspace.focusPane(id);
  }, [maximizedId, workspace.focusPane]);

  useEffect(() => {
    if (voice.state !== "recording" || !voice.recordingId) return;
    const recordingId = voice.recordingId;
    let active = true;
    let timer = 0;

    const sample = async () => {
      try {
        const response = await voiceInputStatus(recordingId);
        if (!active || response.state !== "recording") return;
        const level = Math.max(0, Math.min(1, response.audioLevel ?? 0));
        setVoice((current) => (
          current.state === "recording" && current.recordingId === recordingId
            ? { ...current, levels: [...current.levels, level].slice(-48) }
            : current
        ));
      } catch {
        // Meter updates are best-effort; recording lifecycle errors are handled separately.
      }
      if (active) timer = window.setTimeout(sample, 65);
    };

    void sample();
    return () => {
      active = false;
      window.clearTimeout(timer);
    };
  }, [voice.recordingId, voice.state]);

  const pollVoiceStatus = useCallback(async (recordingId: string, operation: number) => {
    let lastError: unknown = null;
    for (let attempt = 0; attempt < 600; attempt += 1) {
      await sleep(250);
      if (voiceOperation.current !== operation) return;

      let response;
      try {
        response = await voiceInputStatus(recordingId);
        lastError = null;
      } catch (error) {
        lastError = error;
        continue;
      }
      if (voiceOperation.current !== operation) return;

      if (response.state === "completed") {
        const transcript = response.transcript ?? "";
        setVoice((current) => ({
          ...current,
          state: "preview",
          transcript,
          error: null,
        }));
        return transcript;
      }
      if (response.state === "failed" || response.state === "cancelled") {
        throw new Error(response.error ?? `Voice input ${response.state}.`);
      }
    }

    if (lastError) throw lastError;
    throw new Error("Dictator transcription timed out.");
  }, []);

  const advanceVoice = useCallback(async (activationSource?: VoiceActivationSource) => {
    if (voice.state === "connecting" || (voice.state === "transcribing" && !voice.error)) return;

    if (voice.state === "idle") {
      const destination = workspace.sessions.find(
        (session) => session.id === activeId && session.status === "live",
      );
      if (!destination) {
        setControllerNotice("Select a connected terminal before starting voice input.");
        return;
      }
      if (voiceStartInFlight.current) return;
      voiceStartInFlight.current = true;
      const operation = voiceOperation.current + 1;
      voiceOperation.current = operation;
      setControllerNotice(null);
      setVoice({
        state: "connecting",
        activationSource: activationSource ?? "controller",
        recordingId: null,
        destinationId: destination.id,
        transcript: "",
        error: null,
        levels: [],
      });
      try {
        const response = await withTimeout(
          startVoiceInput(),
          15_000,
          "UltraTerm could not connect to Dictator.",
          (lateResponse) => {
            if (lateResponse.recordingId) void cancelVoiceInput(lateResponse.recordingId);
          },
        );
        const recordingId = response.recordingId;
        if (!recordingId) throw new Error("Dictator did not return a recording id.");
        if (voiceOperation.current !== operation) {
          await cancelVoiceInput(recordingId);
          return;
        }
        const initialLevel = response.audioLevel === null ? null : Math.max(0, Math.min(1, response.audioLevel));
        const nextVoice = {
          activationSource: activationSource ?? "controller",
          recordingId,
          destinationId: destination.id,
          transcript: response.transcript ?? "",
          error: null,
          levels: initialLevel === null ? [] : [initialLevel],
        };
        if (response.state === "recording") {
          setVoice({ ...nextVoice, state: "recording" });
          return;
        }
        if (response.state === "transcribing") {
          setVoice({ ...nextVoice, state: "transcribing" });
          try {
            await pollVoiceStatus(recordingId, operation);
          } catch (error) {
            if (voiceOperation.current !== operation) return;
            const message = error instanceof Error ? error.message : String(error);
            setVoice((current) => (
              current.recordingId === recordingId
                ? { ...current, state: "transcribing", error: message }
                : current
            ));
            setControllerNotice(message);
          }
          return;
        }
        if (response.state === "completed") {
          setVoice({ ...nextVoice, state: "preview" });
          return;
        }
        throw new Error(response.error ?? `Voice input ${response.state}.`);
      } catch (error) {
        if (voiceOperation.current !== operation) return;
        setVoice(IDLE_VOICE_SESSION);
        setControllerNotice(error instanceof Error ? error.message : String(error));
      }
      finally {
        voiceStartInFlight.current = false;
      }
      return;
    }

    if (
      (voice.state === "recording" || (voice.state === "transcribing" && voice.error))
      && voice.recordingId
    ) {
      const operation = voiceOperation.current;
      const recordingId = voice.recordingId;
      const shouldStopRecording = voice.state === "recording";
      const shouldInsertAndClose = voice.activationSource === "keyboard";
      let stopCompleted = !shouldStopRecording;
      setVoice((current) => ({ ...current, state: "transcribing", error: null }));
      try {
        if (shouldStopRecording) {
          await finishVoiceInput(recordingId);
          stopCompleted = true;
        }
        const transcript = (await pollVoiceStatus(recordingId, operation)) ?? "";
        if (shouldInsertAndClose) {
          const destination = workspace.sessions.find(
            (session) => session.id === voice.destinationId && session.status === "live",
          );
          if (!destination) throw new Error("The selected terminal disconnected.");
          if (!transcript.trim()) throw new Error("No speech detected.");
          const inserted = await workspace.sendTerminalInput(destination.id, `${transcript}\r`);
          if (!inserted) throw new Error("The transcript could not be inserted.");
          voiceOperation.current += 1;
          setVoice(IDLE_VOICE_SESSION);
          workspace.focusPane(destination.id);
        }
      } catch (error) {
        if (voiceOperation.current !== operation) return;
        let message = error instanceof Error ? error.message : String(error);
        if (shouldInsertAndClose) {
          if (!stopCompleted) {
            try {
              await cancelVoiceInput(recordingId);
            } catch (cancelError) {
              const cancelMessage = cancelError instanceof Error
                ? cancelError.message
                : String(cancelError);
              message = `${message} The recording also could not be cancelled: ${cancelMessage}`;
            }
          }
          voiceOperation.current += 1;
          setVoice(IDLE_VOICE_SESSION);
          setControllerNotice(message);
          const focusId = voice.destinationId ?? activeId;
          if (focusId) workspace.focusPane(focusId);
          return;
        }
        setVoice((current) => (
          current.recordingId === recordingId
            ? {
                ...current,
                state: shouldStopRecording ? "recording" : "transcribing",
                error: message,
              }
            : current
        ));
        setControllerNotice(message);
      }
      return;
    }

    if (voice.state === "preview") {
      const text = voice.transcript;
      const destination = workspace.sessions.find(
        (session) => session.id === voice.destinationId && session.status === "live",
      );
      if (!destination) {
        setVoice((current) => ({
          ...current,
          error: "The selected terminal disconnected. Reconnect it before inserting.",
        }));
        return;
      }
      if (!text.trim()) {
        setVoice((current) => ({ ...current, error: "The transcript is empty." }));
        return;
      }
      if (voiceInsertion.current) return;
      const operation = voiceOperation.current;
      voiceInsertion.current = true;
      setVoiceInserting(true);
      try {
        const inserted = await workspace.sendTerminalInput(destination.id, `${text}\r`);
        if (voiceOperation.current !== operation) return;
        if (!inserted) {
          setVoice((current) => ({
            ...current,
            error: "The transcript could not be inserted. Reconnect the terminal and retry.",
          }));
          return;
        }
        voiceOperation.current += 1;
        setVoice(IDLE_VOICE_SESSION);
        workspace.focusPane(destination.id);
      } finally {
        voiceInsertion.current = false;
        setVoiceInserting(false);
      }
    }
  }, [
    activeId,
    pollVoiceStatus,
    voice,
    workspace.focusPane,
    workspace.sendTerminalInput,
    workspace.sessions,
  ]);

  const cancelVoice = useCallback(async () => {
    if (voiceCancellation.current) return;
    const cancelled = voice;
    const operation = voiceOperation.current + 1;
    voiceOperation.current = operation;
    setVoice(IDLE_VOICE_SESSION);
    const focusId = cancelled.destinationId ?? activeId;
    if (focusId) workspace.focusPane(focusId);

    if (
      cancelled.recordingId
      && (cancelled.state === "recording" || cancelled.state === "transcribing")
    ) {
      voiceCancellation.current = true;
      try {
        await cancelVoiceInput(cancelled.recordingId);
      } catch (error) {
        if (voiceOperation.current !== operation) return;
        const message = error instanceof Error ? error.message : String(error);
        setControllerNotice(message);
      } finally {
        voiceCancellation.current = false;
      }
    }
  }, [activeId, voice, workspace.focusPane]);

  const handleCircle = useCallback((id: string) => {
    if (voice.state !== "idle") {
      void cancelVoice();
      return;
    }
    const input = workspace.hasPendingInput(id) ? "\u0003" : "\u001b";
    void workspace.sendTerminalInput(id, input);
  }, [cancelVoice, voice.state, workspace.hasPendingInput, workspace.sendTerminalInput]);

  const advanceVoiceRef = useRef(advanceVoice);
  advanceVoiceRef.current = advanceVoice;
  const sendTerminalInputRef = useRef(workspace.sendTerminalInput);
  sendTerminalInputRef.current = workspace.sendTerminalInput;

  useEffect(() => {
    let holdTimer: number | null = null;
    let held = false;
    let triggered = false;
    let destinationId: string | null = null;

    const terminalOwnsKeyboardEvent = (event: KeyboardEvent) => (
      !controllerOpen
      && voiceRef.current.state === "idle"
      && Boolean(activeId)
      && (
        (event.target instanceof Element && Boolean(event.target.closest(".terminal-pane")))
        || (
          document.activeElement instanceof Element
          && Boolean(document.activeElement.closest(".terminal-pane"))
        )
      )
    );

    const handleKeyDown = (event: KeyboardEvent) => {
      const isDictatorShortcut = (
        event.code === "Backquote"
        && event.altKey
        && !event.metaKey
        && !event.ctrlKey
        && !event.shiftKey
      );
      if (isDictatorShortcut) {
        event.preventDefault();
        event.stopImmediatePropagation();
        if (event.repeat) return;
        if (voiceRef.current.state !== "idle") {
          void advanceVoiceRef.current();
        } else if (terminalOwnsKeyboardEvent(event)) {
          void advanceVoiceRef.current("keyboard");
        }
        return;
      }

      if (
        (event.code !== "Space" && event.key !== " ")
        || event.metaKey
        || event.ctrlKey
        || event.altKey
        || event.shiftKey
        || !terminalOwnsKeyboardEvent(event)
      ) return;

      event.preventDefault();
      event.stopImmediatePropagation();
      if (event.repeat || held) return;

      held = true;
      triggered = false;
      destinationId = activeId;
      holdTimer = window.setTimeout(() => {
        holdTimer = null;
        if (!held) return;
        triggered = true;
        void advanceVoiceRef.current("keyboard");
      }, SPACE_HOLD_MILLISECONDS);
    };

    const handleSpaceUp = (event: KeyboardEvent) => {
      if ((event.code !== "Space" && event.key !== " ") || !held) return;
      event.preventDefault();
      event.stopImmediatePropagation();
      if (holdTimer !== null) window.clearTimeout(holdTimer);
      if (!triggered && destinationId) {
        void sendTerminalInputRef.current(destinationId, " ");
      }

      holdTimer = null;
      held = false;
      triggered = false;
      destinationId = null;
    };

    const cancelSpaceHold = () => {
      if (holdTimer !== null) window.clearTimeout(holdTimer);
      holdTimer = null;
      held = false;
      triggered = false;
      destinationId = null;
    };

    window.addEventListener("keydown", handleKeyDown, true);
    window.addEventListener("keyup", handleSpaceUp, true);
    window.addEventListener("blur", cancelSpaceHold);
    return () => {
      cancelSpaceHold();
      window.removeEventListener("keydown", handleKeyDown, true);
      window.removeEventListener("keyup", handleSpaceUp, true);
      window.removeEventListener("blur", cancelSpaceHold);
    };
  }, [activeId, controllerOpen]);

  const controller = usePs4Controller({
    activeId,
    sessionIds: workspace.sessions.map((session) => session.id),
    onSelect: (id) => {
      if (!controllerOpen && voice.state === "idle") selectTerminal(id);
    },
    onScrollLines: (id, lines) => {
      if (!controllerOpen && voice.state === "idle") workspace.scrollPane(id, lines);
    },
    onScrollPages: (id, pages) => {
      if (!controllerOpen && voice.state === "idle") workspace.scrollPanePages(id, pages);
    },
    onToggleMaximize: (id) => {
      if (!controllerOpen && voice.state === "idle") {
        setMaximizedId((current) => current === id ? null : id);
      }
    },
    onClose: (id) => {
      if (!controllerOpen && voice.state === "idle") requestCloseTerminals([id]);
    },
    onNewSession: (id) => {
      if (!controllerOpen && voice.state === "idle") {
        void workspace.sendOmpCommand(id, "/new");
      }
    },
    onResume: (id) => {
      if (!controllerOpen && voice.state === "idle") {
        void workspace.sendOmpCommand(id, "/resume");
      }
    },
    onCross: () => {
      void advanceVoice("controller");
    },
    onCircle: (id) => {
      if (voice.state !== "idle") void cancelVoice();
      else if (controllerOpen) setControllerOpen(false);
      else handleCircle(id);
    },
    onOpenControls: () => {
      if (voice.state === "idle") setControllerOpen(true);
    },
  });

  const voiceDestination = voice.destinationId
    ? workspace.sessions.find((session) => session.id === voice.destinationId)
    : null;

  const grid = paneGrid(workspace.sessions.length);
  const gridStyle = {
    "--pane-count": workspace.sessions.length,
    "--pane-columns": grid.columns,
    "--pane-rows": grid.rows,
  } as CSSProperties;
  const splashStatus = workspace.isBooting
    ? "Restoring your workspace…"
    : workspaceReady
      ? "Ready"
      : "Preparing terminals…";


  return (
    <div
      className={`ultraterm-shell effects-${effectMode}`}
      data-theme={theme}
      onMouseDown={(event) => {
        if (event.button !== 0 || event.target !== event.currentTarget) return;
        void getCurrentWindow().startDragging();
      }}
    >
      <AmbientField mode={effectMode} />

      <WorkspaceRail
        sessions={workspace.sessions}
        activeId={activeId}
        selectedIds={selectedIds}
        maximizedId={maximizedId}
        metrics={workspace.metrics}
        telemetry={workspace.telemetry}
        launchProfile={workspace.launchProfile}
        isBooting={workspace.isBooting || workspace.isAddingPane}
        theme={theme}
        effectMode={effectMode}
        terminalPreferences={terminalPreferences}
        notice={workspace.notice ?? controllerNotice}
        controllerVoiceState={voice.state}
        controllerConnected={controller.connected}
        controllerName={controller.controllerName}
        onSelect={selectTerminal}
        onAddTerminal={addTerminal}
        onToggleMaximize={(id) => setMaximizedId((current) => current === id ? null : id)}
        onRestart={(id) => void workspace.restartPane(id)}
        onCloseSelected={closeSelectedTerminals}
        onNewSession={(id) => void workspace.sendOmpCommand(id, "/new")}
        onExitSession={(id) => void workspace.sendOmpCommand(id, "/exit")}
        onThemeChange={setTheme}
        onOpenController={() => setControllerOpen(true)}
        onEffectModeChange={setEffectMode}
        onTerminalPreferencesChange={setTerminalPreferences}
        onDismissNotice={() => {
          workspace.dismissNotice();
          setControllerNotice(null);
        }}
      />

      <main className="workspace">
        <section
          ref={workspaceGridRef}
          className={`workspace-grid workspace-grid--count-${workspace.sessions.length}${layoutMaximizedId ? " has-maximized" : ""}`}
          style={gridStyle}
          aria-label="OMP terminals"
        >
          {workspace.sessions.map((session) => (
            <TerminalPane
              key={session.id}
              session={session}
              scrollback={TERMINAL_SCROLLBACK}
              preferences={terminalPreferences}
              active={session.id === activeId}
              maximized={session.id === layoutMaximizedId}
              exiting={exitingIds.has(session.id)}
              reducedMotion={prefersReducedMotion}
              suppressEntrance={splash !== "gone"}
              onActivate={selectTerminal}
              onControllerReady={handleControllerReady}
              onRestart={(id) => void workspace.restartPane(id)}
            />
          ))}

          {workspace.sessions.length === 0 && (
            <div className="workspace-empty">
              <span className="workspace-empty__lens" aria-hidden="true" />
              <div>
                <p className="eyebrow">TERMINALS OFFLINE</p>
                <h1>{workspace.isBooting ? "Opening your OMP workspace…" : "No terminals connected"}</h1>
                <p>
                  {workspace.isBooting
                    ? `Attaching ${DEFAULT_TERMINAL_COUNT} persistent OMP terminals.`
                    : "Restore your three persistent OMP terminals."}
                </p>
                {!workspace.isBooting && (
                  <button
                    type="button"
                    className="primary-button"
                    onClick={() => void workspace.rebalance(DEFAULT_TERMINAL_COUNT)}
                  >
                    Restore terminals
                  </button>
                )}
              </div>
            </div>
          )}
        </section>
      </main>
      {voice.state !== "idle" && (
        <VoicePreviewModal
          state={voice.state}
          activationSource={voice.activationSource ?? "controller"}
          transcript={voice.transcript}
          error={voice.error}
          advancing={voiceInserting}
          levels={voice.levels}
          terminalLabel={voiceDestination ? `Terminal ${voiceDestination.slot}` : "Selected terminal"}
          onTranscriptChange={(transcript) => setVoice((current) => ({
            ...current,
            transcript,
            error: null,
          }))}
          onAdvance={() => void advanceVoice()}
          onCancel={() => void cancelVoice()}
        />
      )}

      <ControllerModal
        open={controllerOpen}
        connected={controller.connected}
        controllerName={controller.controllerName}
        voiceState={voice.state}
        onToggleVoice={() => {
          setControllerOpen(false);
          void advanceVoice("controller");
        }}
        onClose={() => setControllerOpen(false)}
      />

      {splash !== "gone" && (
        <div
          className={`boot-splash${splash === "exiting" ? " is-exiting" : ""}`}
          role="status"
          aria-label="Loading UltraTerm"
          onTransitionEnd={(event) => {
            if (event.target === event.currentTarget && splash === "exiting") setSplash("gone");
          }}
        >
          <div className="boot-splash__card">
            <strong className="boot-splash__wordmark">UltraTerm</strong>
            <small className="boot-splash__byline">By Implose Labs</small>
            <span className="boot-splash__bar" aria-hidden="true"><span /></span>
            <p className="boot-splash__status">{splashStatus}</p>
          </div>
        </div>
      )}
    </div>
  );
}

export default App;
