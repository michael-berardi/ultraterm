// Boot diagnostics: surface fatal module/boot errors in the splash status
// line instead of leaving the splash up forever with no explanation.
function bootStatusElement(): HTMLElement | null {
  return (
    document.querySelector<HTMLElement>(".boot-splash-inline__status") ??
    document.querySelector<HTMLElement>(".boot-splash__status")
  );
}

function showBootError(message: string): void {
  const el = bootStatusElement();
  if (el) el.textContent = `Boot error: ${message}`;
  console.error(`[ultraterm] boot error: ${message}`);
}

window.addEventListener("error", (event) => {
  showBootError(event.message || event.type);
});

window.addEventListener("unhandledrejection", (event) => {
  const reason = event.reason;
  showBootError(
    reason instanceof Error ? reason.message : String(reason ?? "unhandled rejection"),
  );
});

// Immediate marker that the JS bundle evaluated at all.
const marker = bootStatusElement();
if (marker) marker.textContent = "Starting UltraTerm…";
