import type { EffectMode } from "../types";

interface AmbientFieldProps {
  mode: EffectMode;
}

/**
 * Slow prismatic light field behind the workspace. Only the spectrum effect
 * renders anything; off/focus keep the canvas pure black.
 */
export function AmbientField({ mode }: AmbientFieldProps) {
  if (mode !== "spectrum") return null;

  return (
    <div className="ambient-field" aria-hidden="true">
      <span className="ambient-field__caustic ambient-field__caustic--one" />
      <span className="ambient-field__caustic ambient-field__caustic--two" />
      <span className="ambient-field__prism" />
    </div>
  );
}
