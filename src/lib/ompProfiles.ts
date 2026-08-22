import type { CreateOmpProfileRequest, OmpProfileInfo, OmpThinkingLevel } from "../types";

/**
 * Profile names are lowercase ASCII alphanumeric plus internal hyphens,
 * 1–48 chars (mirrors the backend safety rules).
 */
const OMP_PROFILE_NAME_PATTERN = /^[a-z0-9]+(?:-[a-z0-9]+)*$/;

export interface CreateOmpProfileFields {
  name: string;
  model: string;
  thinkingLevel: OmpThinkingLevel;
  titleModel: string;
}

export type CreateOmpProfilePayload =
  | { request: CreateOmpProfileRequest; error?: never }
  | { request?: never; error: string };

/** Validates the create form and builds the exact wire payload. */
export function buildCreateOmpProfileRequest(fields: CreateOmpProfileFields): CreateOmpProfilePayload {
  const name = fields.name.trim();
  const model = fields.model.trim();
  const titleModel = fields.titleModel.trim();
  if (!OMP_PROFILE_NAME_PATTERN.test(name) || name.length > 48) {
    return { error: "Profile names use 1–48 lowercase letters, digits, and internal hyphens." };
  }
  if (!model) {
    return { error: "Enter the exact model ID this profile should use." };
  }
  return {
    request: {
      name,
      model,
      thinkingLevel: fields.thinkingLevel,
      ...(titleModel ? { titleModel } : {}),
    },
  };
}

/** Active profiles cannot be removed. */
export function canRemoveOmpProfile(profile: OmpProfileInfo): boolean {
  return !profile.active;
}

export interface ProfileRemovalState {
  /** Name whose inline confirmation is armed, if any. */
  armedName: string | null;
  /** True when this click is the second step and removal should proceed. */
  confirmed: boolean;
}

/** Two-step inline removal: first click arms the row, second click confirms. */
export function advanceProfileRemoval(armedName: string | null, name: string): ProfileRemovalState {
  if (armedName === name) return { armedName: null, confirmed: true };
  return { armedName: name, confirmed: false };
}

