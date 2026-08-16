import { useCallback, useEffect, useRef, useState } from "react";
import { checkAppUpdate, installAppUpdate } from "../lib/terminalApi";
import type { AppUpdateStatus } from "../types";

const AUTO_UPDATE_STORAGE_KEY = "ultraterm.auto-update";

export type UpdatePhase = "idle" | "prompt" | "installing" | "error";

export function readAutoUpdatePreference(): boolean {
  try {
    return localStorage.getItem(AUTO_UPDATE_STORAGE_KEY) === "1";
  } catch {
    return false;
  }
}

/**
 * GitHub stable-release checks on launch and every 24 hours. When a newer
 * version exists, the auto-update preference decides between an immediate
 * verified install and a prompt. Check failures are swallowed: an offline
 * check must never block the workspace.
 */
export function useAppUpdate() {
  const [status, setStatus] = useState<AppUpdateStatus | null>(null);
  const [phase, setPhase] = useState<UpdatePhase>("idle");
  const [error, setError] = useState<string | null>(null);
  const [autoUpdate, setAutoUpdateState] = useState(readAutoUpdatePreference);
  const started = useRef(false);

  const setAutoUpdate = useCallback((value: boolean): void => {
    setAutoUpdateState(value);
    try {
      localStorage.setItem(AUTO_UPDATE_STORAGE_KEY, value ? "1" : "0");
    } catch {
      // Preference persistence is best-effort.
    }
  }, []);

  const install = useCallback(async (): Promise<void> => {
    setPhase("installing");
    setError(null);
    try {
      await installAppUpdate();
      // Success exits the process; the detached helper swaps the bundle and
      // relaunches into the new version.
    } catch (installError) {
      setError(installError instanceof Error ? installError.message : String(installError));
      setPhase("error");
    }
  }, []);

  useEffect(() => {
    if (started.current) return;
    started.current = true;
    let cancelled = false;
    let checking = false;
    const check = async (): Promise<void> => {
      if (checking) return;
      checking = true;
      try {
        const result = await checkAppUpdate();
        if (cancelled || !result.updateAvailable) return;
        setStatus(result);
        if (readAutoUpdatePreference()) void install();
        else setPhase("prompt");
      } catch {
        // Best-effort check; never interrupt terminal work.
      } finally {
        checking = false;
      }
    };
    void check();
    const interval = window.setInterval(() => void check(), 24 * 60 * 60 * 1000);
    return () => {
      cancelled = true;
      window.clearInterval(interval);
    };
  }, [install]);

  const dismiss = useCallback((): void => {
    setPhase("idle");
    setError(null);
  }, []);

  return { status, phase, error, autoUpdate, setAutoUpdate, install, dismiss };
}
