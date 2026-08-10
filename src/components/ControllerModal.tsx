import { useEffect, useRef } from "react";
import { createPortal } from "react-dom";
import { Gamepad2, Mic, MicOff, X } from "lucide-react";
import { PS4_CONTROL_MAP } from "../hooks/usePs4Controller";
import type { VoiceInputState } from "../types";

interface ControllerModalProps {
  open: boolean;
  connected: boolean;
  controllerName: string | null;
  voiceState: VoiceInputState;
  onToggleVoice: () => void;
  onClose: () => void;
}

export function ControllerModal({
  open,
  connected,
  controllerName,
  voiceState,
  onToggleVoice,
  onClose,
}: ControllerModalProps) {
  const modalRef = useRef<HTMLDivElement>(null);
  const closeButtonRef = useRef<HTMLButtonElement>(null);
  const previousFocusRef = useRef<HTMLElement | null>(null);

  useEffect(() => {
    if (!open) return;
    previousFocusRef.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const frame = requestAnimationFrame(() => closeButtonRef.current?.focus());
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        onClose();
        return;
      }
      if (event.key !== "Tab") return;
      const focusable = Array.from(
        modalRef.current?.querySelectorAll<HTMLElement>('button:not([disabled]), [href], [tabindex]:not([tabindex="-1"])') ?? [],
      );
      if (focusable.length === 0) return;
      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first.focus();
      }
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => {
      cancelAnimationFrame(frame);
      window.removeEventListener("keydown", handleKeyDown);
      previousFocusRef.current?.focus();
    };
  }, [onClose, open]);

  const voiceActive = voiceState !== "idle";
  const voiceDetail = voiceState === "recording"
    ? "Recording — press X again to transcribe"
    : voiceState === "transcribing" || voiceState === "connecting"
      ? "Preparing your transcript"
      : voiceState === "preview"
        ? "Preview ready — press X to insert"
        : "Ready — press X to start";

  if (!open) return null;

  return createPortal(
    <div className="controller-overlay" role="presentation" onMouseDown={(event) => {
      if (event.target === event.currentTarget) onClose();
    }}>
      <div ref={modalRef} className="controller-modal" role="dialog" aria-modal="true" aria-labelledby="controller-title">
        <header>
          <div>
            <span className={`controller-status-dot${connected ? " is-connected" : ""}`} aria-hidden="true" />
            <small>{connected ? "Connected" : "Waiting for controller"}</small>
            <h2 id="controller-title"><Gamepad2 size={19} /> PS4 controls</h2>
            <p>{controllerName ?? "Connect a DualShock 4, then press any button."}</p>
          </div>
          <button ref={closeButtonRef} type="button" className="appearance-close" aria-label="Close controller controls" onClick={onClose}>
            <X size={16} />
          </button>
        </header>

        <div className="controller-map">
          {PS4_CONTROL_MAP.map(({ control, action }) => (
            <div key={control} className="controller-map__row">
              <kbd>{control}</kbd>
              <span>{action}</span>
            </div>
          ))}
        </div>

        <footer className="controller-voice-status">
          <div>
            {voiceActive ? <Mic size={15} /> : <MicOff size={15} />}
            <span>
              <strong>UltraVox Voice</strong>
              <small>{voiceDetail}</small>
            </span>
          </div>
          <button type="button" onClick={onToggleVoice} disabled={voiceState === "connecting" || voiceState === "transcribing"}>
            {voiceState === "recording" ? "Stop voice" : voiceState === "preview" ? "Insert" : voiceActive ? "Working…" : "Start voice"}
          </button>
        </footer>
      </div>
    </div>,
    document.body,
  );
}
