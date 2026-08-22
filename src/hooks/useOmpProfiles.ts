import { useCallback, useEffect, useState } from "react";
import { isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  createOmpProfile,
  listOmpProfiles,
  removeOmpProfile,
} from "../lib/terminalApi";
import type { CreateOmpProfileRequest, OmpProfileInfo } from "../types";

function profileListError(error: unknown): string {
  return error instanceof Error && error.message
    ? error.message
    : "UltraTerm could not load OMP profiles.";
}

export function useOmpProfiles() {
  const desktopRuntime = isTauri();
  const [profiles, setProfiles] = useState<OmpProfileInfo[]>([]);
  const [profilesLoaded, setProfilesLoaded] = useState(false);
  const [profilesError, setProfilesError] = useState<string | null>(null);

  const refreshProfiles = useCallback(async (): Promise<void> => {
    if (!desktopRuntime) return;
    try {
      const next = await listOmpProfiles();
      setProfiles(next);
      setProfilesError(null);
    } catch (error) {
      setProfilesError(profileListError(error));
      throw error;
    } finally {
      setProfilesLoaded(true);
    }
  }, [desktopRuntime]);

  useEffect(() => {
    if (!desktopRuntime) return;
    void refreshProfiles().catch(() => {});
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void listen("omp-profiles-changed", () => {
      void refreshProfiles().catch(() => {});
    }).then((cleanup) => {
      if (disposed) cleanup();
      else unlisten = cleanup;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [desktopRuntime, refreshProfiles]);

  const createProfile = useCallback(async (request: CreateOmpProfileRequest): Promise<void> => {
    const created = await createOmpProfile(request);
    setProfiles((current) => [...current.filter((profile) => profile.name !== created.name), created]
      .sort((left, right) => left.name.localeCompare(right.name)));
    void refreshProfiles().catch(() => {});
  }, [refreshProfiles]);

  const removeProfile = useCallback(async (name: string): Promise<void> => {
    await removeOmpProfile(name);
    setProfiles((current) => current.filter((profile) => profile.name !== name));
    void refreshProfiles().catch(() => {});
  }, [refreshProfiles]);

  return {
    profiles,
    profilesLoaded,
    profilesError,
    refreshProfiles,
    createProfile,
    removeProfile,
  };
}
