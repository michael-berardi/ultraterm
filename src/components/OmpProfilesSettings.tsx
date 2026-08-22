import { useEffect, useRef, useState, type FormEvent, type ReactElement } from "react";
import { Trash2 } from "lucide-react";
import {
  advanceProfileRemoval,
  buildCreateOmpProfileRequest,
  canRemoveOmpProfile,
} from "../lib/ompProfiles";
import {
  OMP_THINKING_LEVELS,
  type CreateOmpProfileRequest,
  type OmpProfileInfo,
  type OmpThinkingLevel,
} from "../types";

interface OmpProfilesSettingsProps {
  profiles: OmpProfileInfo[];
  profilesLoaded: boolean;
  profilesError: string | null;
  onRefreshProfiles: () => Promise<void>;
  onCreateProfile: (request: CreateOmpProfileRequest) => Promise<void>;
  onRemoveProfile: (name: string) => Promise<void>;
}

function profileErrorMessage(error: unknown): string {
  if (typeof error === "string") return error;
  if (error instanceof Error && error.message) return error.message;
  return "The profile operation failed. Check the name and try again.";
}

function ProfileRow({
  profile,
  armedName,
  removingName,
  busy,
  onArm,
  onCancel,
  onConfirm,
}: {
  profile: OmpProfileInfo;
  armedName: string | null;
  removingName: string | null;
  busy: boolean;
  onArm: (name: string) => void;
  onCancel: () => void;
  onConfirm: (name: string) => void;
}): ReactElement {
  const armed = armedName === profile.name;
  const removing = removingName === profile.name;
  const removable = canRemoveOmpProfile(profile);
  const confirmButtonRef = useRef<HTMLButtonElement>(null);
  const removeButtonRef = useRef<HTMLButtonElement>(null);
  const wasArmedRef = useRef(false);

  useEffect(() => {
    if (armed) confirmButtonRef.current?.focus();
    else if (wasArmedRef.current) removeButtonRef.current?.focus();
    wasArmedRef.current = armed;
  }, [armed]);

  return (
    <div className="settings-preference-row omp-profile-row">
      <span>
        <strong>{profile.name}</strong>
        <small>
          {profile.active
            ? "In use by a running OMP terminal"
            : "Available in the launch profile menu"}
        </small>
      </span>
      {profile.active ? (
        <span className="omp-profile-row__badge" title="Active profiles cannot be removed">
          Active
        </span>
      ) : armed ? (
        <span
          className="omp-profile-row__confirm"
          role="group"
          aria-label={`Confirm removal of ${profile.name}`}
          onKeyDown={(event) => {
            if (event.key !== "Escape" || removing) return;
            event.preventDefault();
            onCancel();
          }}
        >
          <small>Delete {profile.name} and its saved OMP data?</small>
          <button
            ref={confirmButtonRef}
            type="button"
            className="provider-card__button provider-card__button--danger"
            disabled={removing}
            onClick={() => onConfirm(profile.name)}
          >
            {removing ? "Deleting…" : "Delete"}
          </button>
          <button
            type="button"
            className="provider-card__button"
            disabled={removing}
            onClick={onCancel}
          >
            Cancel
          </button>
        </span>
      ) : (
        <button
          ref={removeButtonRef}
          type="button"
          className="provider-card__button provider-card__button--danger"
          disabled={!removable || busy}
          title={removable ? `Delete the ${profile.name} profile and its saved data` : "Active profiles cannot be removed"}
          aria-label={`Delete ${profile.name} profile and its saved data`}
          onClick={() => onArm(profile.name)}
        >
          <Trash2 size={11} aria-hidden="true" /> Delete
        </button>
      )}
    </div>
  );
}

export function OmpProfilesSettings({
  profiles,
  profilesLoaded,
  profilesError,
  onRefreshProfiles,
  onCreateProfile,
  onRemoveProfile,
}: OmpProfilesSettingsProps): ReactElement {
  const [name, setName] = useState("");
  const [model, setModel] = useState("");
  const [thinkingLevel, setThinkingLevel] = useState<OmpThinkingLevel>("auto");
  const [titleModel, setTitleModel] = useState("");
  const [armedName, setArmedName] = useState<string | null>(null);
  const [pending, setPending] = useState<"create" | "remove" | null>(null);
  const [removingName, setRemovingName] = useState<string | null>(null);
  const [createNotice, setCreateNotice] = useState<{ kind: "ok" | "error"; text: string } | null>(null);
  const [listNotice, setListNotice] = useState<{ kind: "ok" | "error"; text: string } | null>(null);
  const listSectionRef = useRef<HTMLElement>(null);

  const handleCreate = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (pending) return;
    const payload = buildCreateOmpProfileRequest({ name, model, thinkingLevel, titleModel });
    if (payload.request === undefined) {
      setCreateNotice({ kind: "error", text: payload.error });
      return;
    }
    const request = payload.request;
    setPending("create");
    setCreateNotice(null);
    try {
      await onCreateProfile(request);
      setCreateNotice({ kind: "ok", text: `Profile ${request.name} created. It now appears in the launch profile menu.` });
      setName("");
      setModel("");
      setTitleModel("");
      setThinkingLevel("auto");
    } catch (error) {
      setCreateNotice({ kind: "error", text: profileErrorMessage(error) });
    } finally {
      setPending(null);
    }
  };

  const handleRemoveClick = (profileName: string) => {

    const next = advanceProfileRemoval(armedName, profileName);
    if (!next.confirmed) {
      setArmedName(next.armedName);
      return;
    }
    if (pending) return;
    setPending("remove");
    setRemovingName(profileName);
    setListNotice(null);
    void onRemoveProfile(profileName)
      .then(() => {
        setListNotice({ kind: "ok", text: `Profile ${profileName} and its saved data were deleted. Launch menus updated.` });
        setArmedName(null);
        requestAnimationFrame(() => listSectionRef.current?.focus());
      })
      .catch((error) => {
        setListNotice({ kind: "error", text: profileErrorMessage(error) });
      })
      .finally(() => {
        setPending(null);
        setRemovingName(null);
      });
  };

  return (
    <>
      <header className="settings-section-intro">
        <h3>OMP Profiles</h3>
        <p>Named OMP configurations under ~/.omp/profiles. Default OMP always stays available.</p>
      </header>

      <section
        ref={listSectionRef}
        className="settings-group"
        aria-labelledby="omp-profile-list-heading"
        tabIndex={-1}
      >
        <div>
          <h4 id="omp-profile-list-heading">Installed profiles</h4>
          <p>Active profiles cannot be removed. Deleting a profile removes its configuration, saved sessions, and other profile-local data.</p>
        </div>
        {profilesError ? (
          <div>
            <p className="provider-card__notice provider-card__notice--error" role="alert">
              {profilesError}
            </p>
            <button
              type="button"
              className="provider-card__button"
              onClick={() => void onRefreshProfiles().catch(() => {})}
            >
              Retry
            </button>
          </div>
        ) : !profilesLoaded ? (
          <p className="omp-profiles-empty" role="status">Loading profiles…</p>
        ) : profiles.length === 0 ? (
          <p className="omp-profiles-empty">
            No profiles yet. Create one below, or keep using Default OMP.
          </p>
        ) : (
          <div className="settings-preference-group">
            {profiles.map((profile) => (
              <ProfileRow
                key={profile.name}
                profile={profile}
                armedName={armedName}
                removingName={removingName}
                busy={pending !== null}
                onArm={handleRemoveClick}
                onCancel={() => setArmedName(null)}
                onConfirm={handleRemoveClick}
              />
            ))}
          </div>
        )}
        {listNotice && (
          <p
            className={`provider-card__notice provider-card__notice--${listNotice.kind}`}
            role={listNotice.kind === "error" ? "alert" : "status"}
          >
            {listNotice.text}
          </p>
        )}
      </section>

      <section className="provider-card" aria-labelledby="omp-profile-create-title">
        <div className="provider-card__header">
          <div className="provider-card__title">
            <strong id="omp-profile-create-title">New profile</strong>
            <small>Every role in the profile routes to the model you name here.</small>
          </div>
        </div>

        <form className="provider-card__form" onSubmit={(event) => void handleCreate(event)}>
          <div className="provider-card__field">
            <label htmlFor="omp-profile-name">Profile name</label>
            <input
              id="omp-profile-name"
              value={name}
              onChange={(event) => setName(event.currentTarget.value)}
              placeholder="lowercase-letters-digits-hyphens"
              autoComplete="off"
              spellCheck={false}
              maxLength={48}
              disabled={pending !== null}
            />
          </div>

          <div className="provider-card__field">
            <label htmlFor="omp-profile-model">Model</label>
            <input
              id="omp-profile-model"
              value={model}
              onChange={(event) => setModel(event.currentTarget.value)}
              placeholder="Exact model ID"
              autoComplete="off"
              spellCheck={false}
              disabled={pending !== null}
            />
          </div>

          <div className="provider-card__field">
            <label htmlFor="omp-profile-thinking">Reasoning</label>
            <select
              id="omp-profile-thinking"
              value={thinkingLevel}
              onChange={(event) => setThinkingLevel(event.currentTarget.value as OmpThinkingLevel)}
              disabled={pending !== null}
            >
              {OMP_THINKING_LEVELS.map((level) => (
                <option key={level} value={level}>{level}</option>
              ))}
            </select>
          </div>
          <div className="provider-card__field">
            <label htmlFor="omp-profile-title-model">Local title model (optional)</label>
            <input
              id="omp-profile-title-model"
              value={titleModel}
              onChange={(event) => setTitleModel(event.currentTarget.value)}
              placeholder="Exact ID from omp tiny-models list"
              autoComplete="off"
              spellCheck={false}
              disabled={pending !== null}
            />
            <small className="provider-card__field-hint">
              Use a downloaded OMP tiny model when the main endpoint rejects title requests.
            </small>
          </div>

          <div className="provider-card__actions">
            <button
              type="submit"
              className="provider-card__button provider-card__button--primary"
              disabled={pending !== null || name.trim() === "" || model.trim() === ""}
            >
              {pending === "create" ? "Creating…" : "Create profile"}
            </button>
          </div>
        </form>

        {createNotice && (
          <p
            className={`provider-card__notice provider-card__notice--${createNotice.kind}`}
            role={createNotice.kind === "error" ? "alert" : "status"}
          >
            {createNotice.text}
          </p>
        )}
      </section>
    </>
  );
}
