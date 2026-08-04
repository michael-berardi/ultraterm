import { useState, type FormEvent, type ReactElement } from "react";
import {
  formatUpdatedAgo,
  isUsageStale,
  usageStatusLabel,
  useProviderUsage,
} from "../hooks/useProviderUsage";
import type { ProviderUsage, UsageProviderId } from "../types";

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

export function IntegrationsSettings(): ReactElement {
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

      {error && (
        <p className="provider-card__notice provider-card__notice--error" role="alert">
          {error}
        </p>
      )}

      <div className="integrations-list">
        {PROVIDERS.map((meta) => {
          const usage = usages.find((item) => item.provider === meta.id);
          return usage ? <ProviderCard key={meta.id} meta={meta} usage={usage} /> : null;
        })}
      </div>
    </div>
  );
}
