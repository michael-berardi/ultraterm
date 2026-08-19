import { Fragment, useState, type FormEvent, type ReactElement } from "react";
import {
  formatUpdatedAgo,
  isUsageStale,
  usageStatusLabel,
  useProviderUsage,
} from "../hooks/useProviderUsage";
import type {
  ProviderUsage,
  ProviderUsagePreferences,
  UsageProviderId,
} from "../types";

interface ProviderMeta {
  id: UsageProviderId;
  name: string;
  tokenLabel: string;
  tokenHint: string;
  accountId?: { label: string; hint: string };
}

const PROVIDERS: ReadonlyArray<ProviderMeta> = [
  {
    id: "kimi",
    name: "Kimi",
    tokenLabel: "Access token",
    tokenHint: "Bearer token from your Kimi coding dashboard.",
  },
  {
    id: "codex",
    name: "Codex",
    tokenLabel: "Access token",
    tokenHint: "ChatGPT access token used for Codex usage.",
    accountId: {
      label: "Account ID (optional)",
      hint: "ChatGPT account ID, only needed for team workspaces.",
    },
  },
  {
    id: "claude",
    name: "Claude",
    tokenLabel: "OAuth token",
    tokenHint: "Claude OAuth access token from your account.",
  },
  {
    id: "zai",
    name: "ZAI",
    tokenLabel: "API token",
    tokenHint: "Bearer token from your z.ai console.",
  },
];

function submitErrorMessage(error: unknown): string {
  if (typeof error === "string") return error;
  if (error instanceof Error && error.message) return error.message;
  return "The provider rejected the request. Check the token and try again.";
}

/** Decoded payload of a JWT, or null when the token is not a JWT. */
export function decodeJwtPayload(token: string): Record<string, unknown> | null {
  const segment = token.split(".")[1];
  if (!segment) return null;
  try {
    const base64 = segment.replace(/-/g, "+").replace(/_/g, "/");
    const parsed: unknown = JSON.parse(atob(base64));
    return parsed && typeof parsed === "object" ? parsed as Record<string, unknown> : null;
  } catch {
    return null;
  }
}

interface ParsedCodexAuth {
  accessToken: string;
  refreshToken: string;
  accountId?: string;
  email?: string;
  expiresAt?: number;
}

/**
 * Parses the contents of a fallback account's ~/.codex/auth.json into the
 * credential fields UltraTerm stores and syncs into OMP.
 */
export function parseCodexAuthJson(raw: string): ParsedCodexAuth {
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    throw new Error("That does not look like JSON — paste the full contents of ~/.codex/auth.json.");
  }
  const root = parsed && typeof parsed === "object" ? parsed as Record<string, unknown> : {};
  const tokens = root.tokens && typeof root.tokens === "object"
    ? root.tokens as Record<string, unknown>
    : {};
  const accessToken = typeof tokens.access_token === "string" ? tokens.access_token.trim() : "";
  const refreshToken = typeof tokens.refresh_token === "string" ? tokens.refresh_token.trim() : "";
  if (!accessToken || !refreshToken) {
    throw new Error("The auth.json is missing tokens.access_token or tokens.refresh_token.");
  }
  const accountId = typeof tokens.account_id === "string" && tokens.account_id.trim()
    ? tokens.account_id.trim()
    : undefined;
  const idClaims = typeof tokens.id_token === "string" ? decodeJwtPayload(tokens.id_token) : null;
  const email = typeof idClaims?.email === "string" && idClaims.email.trim()
    ? idClaims.email.trim()
    : undefined;
  const accessClaims = decodeJwtPayload(accessToken);
  const expiresAt = typeof accessClaims?.exp === "number" ? accessClaims.exp * 1000 : undefined;
  return { accessToken, refreshToken, accountId, email, expiresAt };
}

function CodexFallbackCard({ usage }: { usage: ProviderUsage }): ReactElement {
  const { connect, disconnect } = useProviderUsage();
  const [authJson, setAuthJson] = useState("");
  const [pending, setPending] = useState<"connect" | "disconnect" | null>(null);
  const [notice, setNotice] = useState<{ kind: "ok" | "error"; text: string } | null>(null);

  const stale = isUsageStale(usage);
  const connected = usage.status !== "disconnected";
  const statusCopy = usage.status === "connected" && !stale
    ? `Connected · ${formatUpdatedAgo(usage.updatedAt)}`
    : usageStatusLabel(usage, stale);

  const handleConnect = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const raw = authJson.trim();
    if (!raw || pending) return;
    setPending("connect");
    setNotice(null);
    try {
      const parsed = parseCodexAuthJson(raw);
      await connect({ provider: "codex-fallback", ...parsed });
      setNotice({
        kind: "ok",
        text: "Fallback connected. Live terminals switch to this account automatically when the primary Codex account runs out of quota.",
      });
    } catch (error) {
      setNotice({ kind: "error", text: submitErrorMessage(error) });
    } finally {
      // Credentials are never retained in component state after submit.
      setAuthJson("");
      setPending(null);
    }
  };

  const handleDisconnect = async () => {
    if (pending) return;
    setPending("disconnect");
    setNotice(null);
    try {
      await disconnect("codex-fallback");
      setNotice({ kind: "ok", text: "Fallback disconnected. Credential removed from the Keychain and from OMP." });
    } catch (error) {
      setNotice({ kind: "error", text: submitErrorMessage(error) });
    } finally {
      setPending(null);
    }
  };

  return (
    <section className="provider-card" aria-labelledby="provider-codex-fallback-title">
      <div className="provider-card__header">
        <div className="provider-card__title">
          <strong id="provider-codex-fallback-title">Codex Fallback</strong>
          <small>
            {[usage.plan, usage.balance].filter(Boolean).join(" · ")
              || "Secondary ChatGPT account. Terminals fail over to it when the primary Codex account is out of quota."}
          </small>
        </div>
        <span className={`provider-card__status provider-card__status--${stale ? "stale" : usage.status}`}>
          {statusCopy}
        </span>
      </div>

      {usage.status === "error" && usage.error && (
        <p className="provider-card__notice provider-card__notice--error" role="alert">
          {usage.error}
        </p>
      )}

      <form className="provider-card__form" onSubmit={(event) => void handleConnect(event)}>
        <div className="provider-card__field">
          <label htmlFor="provider-codex-fallback-auth">Account auth.json</label>
          <textarea
            id="provider-codex-fallback-auth"
            value={authJson}
            onChange={(event) => setAuthJson(event.currentTarget.value)}
            placeholder={connected
              ? "Paste a new auth.json to replace the stored fallback account"
              : "Paste the full contents of ~/.codex/auth.json from the fallback account"}
            autoComplete="off"
            spellCheck={false}
            rows={4}
            disabled={pending !== null}
          />
        </div>

        <div className="provider-card__actions">
          {connected && (
            <button
              type="button"
              className="provider-card__button provider-card__button--danger"
              onClick={() => void handleDisconnect()}
              disabled={pending !== null}
            >
              {pending === "disconnect" ? "Disconnecting…" : "Disconnect"}
            </button>
          )}
          <button
            type="submit"
            className="provider-card__button provider-card__button--primary"
            disabled={pending !== null || authJson.trim() === ""}
          >
            {pending === "connect" ? "Saving…" : connected ? "Update account" : "Connect"}
          </button>
        </div>
      </form>

      {notice && (
        <p
          className={`provider-card__notice provider-card__notice--${notice.kind}`}
          role={notice.kind === "error" ? "alert" : "status"}
        >
          {notice.text}
        </p>
      )}
    </section>
  );
}

function ProviderCard({ meta, usage }: { meta: ProviderMeta; usage: ProviderUsage }): ReactElement {
  const { connect, disconnect } = useProviderUsage();
  const [token, setToken] = useState("");
  const [accountId, setAccountId] = useState("");
  const [pending, setPending] = useState<"connect" | "disconnect" | null>(null);
  const [notice, setNotice] = useState<{ kind: "ok" | "error"; text: string } | null>(null);

  const stale = isUsageStale(usage);
  const connected = usage.status !== "disconnected";
  const statusCopy = usage.status === "connected" && !stale
    ? `Connected · ${formatUpdatedAgo(usage.updatedAt)}`
    : usageStatusLabel(usage, stale);

  const handleConnect = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const accessToken = token.trim();
    if (!accessToken || pending) return;
    setPending("connect");
    setNotice(null);
    try {
      await connect({
        provider: meta.id,
        accessToken,
        accountId: meta.accountId ? accountId.trim() || undefined : undefined,
      });
      setNotice({ kind: "ok", text: `${meta.name} connected. Token stored in the macOS Keychain.` });
    } catch (error) {
      setNotice({ kind: "error", text: submitErrorMessage(error) });
    } finally {
      // Credentials are never retained in component state after submit.
      setToken("");
      setAccountId("");
      setPending(null);
    }
  };

  const handleDisconnect = async () => {
    if (pending) return;
    setPending("disconnect");
    setNotice(null);
    try {
      await disconnect(meta.id);
      setNotice({ kind: "ok", text: `${meta.name} disconnected. Credential removed from the Keychain.` });
    } catch (error) {
      setNotice({ kind: "error", text: submitErrorMessage(error) });
    } finally {
      setPending(null);
    }
  };

  return (
    <section className="provider-card" aria-labelledby={`provider-${meta.id}-title`}>
      <div className="provider-card__header">
        <div className="provider-card__title">
          <strong id={`provider-${meta.id}-title`}>{meta.name}</strong>
          <small>
            {[usage.plan, usage.balance].filter(Boolean).join(" · ") || meta.tokenHint}
          </small>
        </div>
        <span className={`provider-card__status provider-card__status--${stale ? "stale" : usage.status}`}>
          {statusCopy}
        </span>
      </div>

      {usage.status === "error" && usage.error && (
        <p className="provider-card__notice provider-card__notice--error" role="alert">
          {usage.error}
        </p>
      )}

      <form className="provider-card__form" onSubmit={(event) => void handleConnect(event)}>
        <div className="provider-card__field">
          <label htmlFor={`provider-${meta.id}-token`}>{meta.tokenLabel}</label>
          <input
            id={`provider-${meta.id}-token`}
            type="password"
            value={token}
            onChange={(event) => setToken(event.currentTarget.value)}
            placeholder={connected ? "Paste a new token to update" : "Paste token"}
            autoComplete="off"
            spellCheck={false}
            disabled={pending !== null}
          />
        </div>

        {meta.accountId && (
          <div className="provider-card__field">
            <label htmlFor={`provider-${meta.id}-account`}>{meta.accountId.label}</label>
            <input
              id={`provider-${meta.id}-account`}
              type="text"
              value={accountId}
              onChange={(event) => setAccountId(event.currentTarget.value)}
              placeholder={meta.accountId.hint}
              autoComplete="off"
              spellCheck={false}
              disabled={pending !== null}
            />
          </div>
        )}

        <div className="provider-card__actions">
          {connected && (
            <button
              type="button"
              className="provider-card__button provider-card__button--danger"
              onClick={() => void handleDisconnect()}
              disabled={pending !== null}
            >
              {pending === "disconnect" ? "Disconnecting…" : "Disconnect"}
            </button>
          )}
          <button
            type="submit"
            className="provider-card__button provider-card__button--primary"
            disabled={pending !== null || token.trim() === ""}
          >
            {pending === "connect" ? "Saving…" : connected ? "Update token" : "Connect"}
          </button>
        </div>
      </form>

      {notice && (
        <p
          className={`provider-card__notice provider-card__notice--${notice.kind}`}
          role={notice.kind === "error" ? "alert" : "status"}
        >
          {notice.text}
        </p>
      )}
    </section>
  );
}

interface IntegrationsSettingsProps {
  preferences: ProviderUsagePreferences;
  onPreferencesChange: (preferences: ProviderUsagePreferences) => void;
}

const DISPLAY_OPTIONS: ReadonlyArray<{
  id: string;
  key: keyof ProviderUsagePreferences;
  name: string;
  detail: string;
}> = [
  {
    id: "provider-usage-weekly-pace",
    key: "showWeeklyPace",
    name: "Weekly spend target",
    detail: "Compare usage with the even pace needed to reach zero at reset.",
  },
  {
    id: "provider-usage-reset-times",
    key: "showResetTimes",
    name: "Reset times",
    detail: "Show when each visible quota window resets.",
  },
  {
    id: "provider-usage-short-windows",
    key: "showSecondaryWindows",
    name: "Short-term limits",
    detail: "Show five-hour and other secondary quota windows.",
  },
];

export function IntegrationsSettings({
  preferences,
  onPreferencesChange,
}: IntegrationsSettingsProps): ReactElement {
  const { usages, error } = useProviderUsage();

  return (
    <div className="integrations-panel">
      <header className="settings-section-intro">
        <h3>Integrations</h3>
        <p>
          Connect provider accounts to track live quota usage. Tokens are kept in the macOS
          Keychain and are only sent to the matching provider.
        </p>
      </header>

      <section className="settings-group" aria-labelledby="provider-usage-display-heading">
        <div>
          <h4 id="provider-usage-display-heading">Provider usage display</h4>
          <p>Choose which quota details appear in the workspace sidebar.</p>
        </div>
        <div className="settings-preference-group">
          {DISPLAY_OPTIONS.map((option) => (
            <label
              key={option.key}
              className="settings-preference-row settings-preference-row--toggle"
              htmlFor={option.id}
            >
              <span>
                <strong>{option.name}</strong>
                <small>{option.detail}</small>
              </span>
              <span className="settings-switch">
                <input
                  id={option.id}
                  type="checkbox"
                  checked={preferences[option.key]}
                  onChange={(event) => onPreferencesChange({
                    ...preferences,
                    [option.key]: event.currentTarget.checked,
                  })}
                />
                <span aria-hidden="true" />
              </span>
            </label>
          ))}
        </div>
      </section>

      {error && (
        <p className="provider-card__notice provider-card__notice--error" role="alert">
          {error}
        </p>
      )}

      <div className="integrations-list">
        {PROVIDERS.map((meta) => {
          const usage = usages.find((item) => item.provider === meta.id);
          const card = usage ? <ProviderCard key={meta.id} meta={meta} usage={usage} /> : null;
          if (meta.id === "codex") {
            const fallbackUsage = usages.find((item) => item.provider === "codex-fallback");
            return (
              <Fragment key={meta.id}>
                {card}
                {fallbackUsage ? <CodexFallbackCard usage={fallbackUsage} /> : null}
              </Fragment>
            );
          }
          return card;
        })}
      </div>
    </div>
  );
}
