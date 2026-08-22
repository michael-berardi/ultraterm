import { describe, expect, it } from "vitest";
import {
  advanceProfileRemoval,
  buildCreateOmpProfileRequest,
  canRemoveOmpProfile,
} from "./ompProfiles";
import {
  launchProfileFromOmpProfile,
  launchProfileLabel,
  launchProfileOptions,
} from "../types";

describe("launchProfileOptions", () => {
  it("always offers Default OMP first with a null id", () => {
    expect(launchProfileOptions([])).toEqual([
      {
        id: null,
        label: "Default OMP",
        description: "Launch OMP without a profile",
      },
    ]);
  });

  it("appends discovered profiles with labels derived from their names", () => {
    const options = launchProfileOptions([
      { name: "fast-worker", active: true },
      { name: "reviewer", active: false },
    ]);

    expect(options.map((option) => option.id)).toEqual([null, "fast-worker", "reviewer"]);
    expect(options.map((option) => option.label)).toEqual([
      "Default OMP",
      "fast-worker",
      "reviewer",
    ]);
  });
});

describe("launchProfileLabel", () => {
  it("labels null, undefined, and empty profiles as Default OMP", () => {
    expect(launchProfileLabel(null)).toBe("Default OMP");
    expect(launchProfileLabel(undefined)).toBe("Default OMP");
    expect(launchProfileLabel("")).toBe("Default OMP");
  });

  it("labels named profiles with their own name", () => {
    expect(launchProfileLabel("fast-worker")).toBe("fast-worker");
  });
});

describe("launchProfileFromOmpProfile", () => {
  it("restores arbitrary recorded tmux profile names unchanged", () => {
    expect(launchProfileFromOmpProfile("any-user-profile")).toBe("any-user-profile");
    expect(launchProfileFromOmpProfile("fast-worker")).toBe("fast-worker");
  });

  it("maps empty or missing tmux metadata to null", () => {
    expect(launchProfileFromOmpProfile("")).toBeNull();
    expect(launchProfileFromOmpProfile(null)).toBeNull();
    expect(launchProfileFromOmpProfile(undefined)).toBeNull();
  });
});

describe("buildCreateOmpProfileRequest", () => {
  it("builds a trimmed payload and omits a blank title model", () => {
    expect(buildCreateOmpProfileRequest({
      name: "  fast-worker  ",
      model: "  exact/model-id  ",
      thinkingLevel: "high",
      titleModel: "   ",
    })).toEqual({
      request: { name: "fast-worker", model: "exact/model-id", thinkingLevel: "high" },
    });
  });

  it("includes a supplied title model", () => {
    expect(buildCreateOmpProfileRequest({
      name: "fast-worker",
      model: "exact/model-id",
      thinkingLevel: "auto",
      titleModel: "title/model-id",
    })).toEqual({
      request: {
        name: "fast-worker",
        model: "exact/model-id",
        thinkingLevel: "auto",
        titleModel: "title/model-id",
      },
    });
  });

  it("rejects names outside the lowercase alphanumeric plus internal-hyphen rule", () => {
    const invalid = ["", "Fast", "-lead", "trail-", "double--hyphen", "has.dot", "has space", "x".repeat(49)];
    for (const name of invalid) {
      expect(buildCreateOmpProfileRequest({
        name,
        model: "exact/model-id",
        thinkingLevel: "auto",
        titleModel: "",
      }).error).toMatch(/Profile names/);
    }
  });

  it("accepts single-char and 48-char names", () => {
    for (const name of ["a", "x".repeat(48)]) {
      expect(buildCreateOmpProfileRequest({
        name,
        model: "exact/model-id",
        thinkingLevel: "off",
        titleModel: "",
      }).request?.name).toBe(name);
    }
  });

  it("rejects a blank model", () => {
    expect(buildCreateOmpProfileRequest({
      name: "fast-worker",
      model: "   ",
      thinkingLevel: "auto",
      titleModel: "",
    }).error).toMatch(/model ID/);
  });
});

describe("canRemoveOmpProfile", () => {
  it("disables removal for the active profile", () => {
    expect(canRemoveOmpProfile({ name: "fast-worker", active: true })).toBe(false);
  });

  it("allows removal of inactive profiles", () => {
    expect(canRemoveOmpProfile({ name: "fast-worker", active: false })).toBe(true);
  });
});

describe("advanceProfileRemoval", () => {
  it("requires a second click on the same profile before confirming", () => {
    const armed = advanceProfileRemoval(null, "fast-worker");
    expect(armed).toEqual({ armedName: "fast-worker", confirmed: false });

    const confirmed = advanceProfileRemoval(armed.armedName, "fast-worker");
    expect(confirmed).toEqual({ armedName: null, confirmed: true });
  });

  it("re-arms instead of confirming when a different profile is clicked", () => {
    const next = advanceProfileRemoval("fast-worker", "reviewer");
    expect(next).toEqual({ armedName: "reviewer", confirmed: false });
  });
});

