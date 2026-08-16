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
 * One-shot GitHub release check on launch. When a newer version exists, the
 * auto-update preference decides between an immediate silent install and a
 * prompt. Check failures are swallowed: an offline launch must never block
 * the workspace.
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
    void checkAppUpdate()
      .then((result) => {
        if (cancelled || !result.updateAvailable) return;
        setStatus(result);
        if (readAutoUpdatePreference()) void install();
        else setPhase("prompt");
      })
      .catch(() => {
        // Best-effort check; never interrupt boot.
      });
    return () => {
      cancelled = true;
    };
  }, [install]);

  const dismiss = useCallback((): void => {
    setPhase("idle");
    setError(null);
  }, []);

  return { status, phase, error, autoUpdate, setAutoUpdate, install, dismiss };
}
