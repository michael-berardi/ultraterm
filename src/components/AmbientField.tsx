import type { EffectMode } from "../types";

interface AmbientFieldProps {
  mode: EffectMode;
}

export function AmbientField({ mode }: AmbientFieldProps) {
  if (mode === "off" || mode === "focus") return null;

  return (
    <div className={`ambient-field ambient-field--${mode}`} aria-hidden="true">
      <span className="ambient-field__caustic ambient-field__caustic--one" />
      <span className="ambient-field__caustic ambient-field__caustic--two" />
      <span className="ambient-field__caustic ambient-field__caustic--three" />
      <span className="ambient-field__prism" />
    </div>
  );
}
