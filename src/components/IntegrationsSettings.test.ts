import { describe, expect, it } from "vitest";
import { decodeJwtPayload, parseCodexAuthJson } from "./IntegrationsSettings";

function fakeJwt(payload: Record<string, unknown>): string {
  const encode = (value: string) =>
    btoa(value).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
  return `${encode(JSON.stringify({ alg: "RS256" }))}.${encode(JSON.stringify(payload))}.sig`;
}

describe("decodeJwtPayload", () => {
  it("decodes the payload segment of a JWT", () => {
    expect(decodeJwtPayload(fakeJwt({ email: "wife@example.com" }))).toMatchObject({
      email: "wife@example.com",
    });
  });

  it("returns null for non-JWT input", () => {
    expect(decodeJwtPayload("not-a-jwt")).toBeNull();
  });
});

describe("parseCodexAuthJson", () => {
  it("extracts the full OAuth token set from a codex auth.json", () => {
    const authJson = JSON.stringify({
      auth_mode: "chatgpt",
      OPENAI_API_KEY: null,
      tokens: {
        id_token: fakeJwt({ email: "wife@example.com" }),
        access_token: fakeJwt({ exp: 1_800_000_000 }),
        refresh_token: "rt.1.abc",
        account_id: "account-123",
      },
      last_refresh: "2026-08-17T00:00:00Z",
    });

    expect(parseCodexAuthJson(authJson)).toEqual({
      accessToken: expect.any(String),
      refreshToken: "rt.1.abc",
      accountId: "account-123",
      email: "wife@example.com",
      expiresAt: 1_800_000_000_000,
    });
  });

  it("rejects input that is not JSON with a paste hint", () => {
    expect(() => parseCodexAuthJson("not json")).toThrow(/auth\.json/);
  });

  it("rejects JSON missing the access or refresh token", () => {
    expect(() => parseCodexAuthJson("{}")).toThrow(/access_token/);
    expect(() =>
      parseCodexAuthJson(JSON.stringify({ tokens: { access_token: "a" } })),
    ).toThrow(/refresh_token/);
  });

  it("tolerates missing optional identity fields", () => {
    const parsed = parseCodexAuthJson(JSON.stringify({
      tokens: { access_token: "opaque-token", refresh_token: "rt" },
    }));
    expect(parsed.accountId).toBeUndefined();
    expect(parsed.email).toBeUndefined();
    expect(parsed.expiresAt).toBeUndefined();
  });
});
