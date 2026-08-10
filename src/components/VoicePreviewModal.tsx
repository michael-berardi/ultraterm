import { useEffect, useRef, type CSSProperties } from "react";
import { createPortal } from "react-dom";
import { Mic, Sparkles, X } from "lucide-react";
import { ThinkingOrb } from "thinking-orbs";
import type { ThemeId, VoiceInputState } from "../types";

interface VoicePreviewModalProps {
  state: Exclude<VoiceInputState, "idle">;
  theme: ThemeId;
  activationSource: "keyboard" | "controller";
  transcript: string;
  error: string | null;
  advancing: boolean;
  levels: number[];
  terminalLabel: string;
  onTranscriptChange: (value: string) => void;
  onAdvance: () => void;
  onCancel: () => void;
}

export function getVoiceModalAction(
  state: VoiceInputState,
  activationSource: "keyboard" | "controller",
  event: { key: string; repeat: boolean; metaKey?: boolean },
): "advance" | "cancel" | null {
  if (event.key === "Escape") return "cancel";
  if (activationSource === "keyboard" && event.key === "Enter" && state !== "connecting") {
    return event.repeat ? null : "advance";
  }
  if (state === "preview" && event.key === "Enter") {
    return event.repeat ? null : "advance";
  }
  return null;
}

export function VoicePreviewModal({
  state,
  theme,
  activationSource,
  transcript,
  error,
  advancing,
  levels,
  terminalLabel,
  onTranscriptChange,
  onAdvance,
  onCancel,
}: VoicePreviewModalProps) {
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const modalRef = useRef<HTMLElement>(null);
  const closeButtonRef = useRef<HTMLButtonElement>(null);
  const previousFocusRef = useRef<HTMLElement | null>(null);
  const stateRef = useRef(state);
  const onAdvanceRef = useRef(onAdvance);
  const onCancelRef = useRef(onCancel);
  stateRef.current = state;
  onAdvanceRef.current = onAdvance;
  onCancelRef.current = onCancel;

  useEffect(() => {
    if (state !== "preview") return;
    const textarea = textareaRef.current;
    textarea?.focus();
    textarea?.setSelectionRange(textarea.value.length, textarea.value.length);
  }, [state]);

  useEffect(() => {
    previousFocusRef.current = document.activeElement instanceof HTMLElement
      ? document.activeElement
      : null;
    const focusFrame = requestAnimationFrame(() => closeButtonRef.current?.focus());
    const handleKeyDown = (event: KeyboardEvent) => {
      const action = getVoiceModalAction(stateRef.current, activationSource, {
        key: event.key,
        repeat: event.repeat,
      });
      if (action === "cancel") {
        event.preventDefault();
        event.stopPropagation();
        onCancelRef.current();
        return;
      }
      if (action === "advance") {
        event.preventDefault();
        event.stopPropagation();
        onAdvanceRef.current();
        return;
      }
      if (event.key !== "Tab" || !modalRef.current) return;

      const focusable = Array.from(
        modalRef.current.querySelectorAll<HTMLElement>(
          "button:not(:disabled), textarea:not(:disabled), [href], [tabindex]:not([tabindex='-1'])",
        ),
      );
      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      if (!first || !last) return;

      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first.focus();
      } else if (!modalRef.current.contains(document.activeElement)) {
        event.preventDefault();
        first.focus();
      }
    };
    window.addEventListener("keydown", handleKeyDown, true);
    return () => {
      cancelAnimationFrame(focusFrame);
      window.removeEventListener("keydown", handleKeyDown, true);
      previousFocusRef.current?.focus();
    };
  }, []);

  const isBusy = state === "connecting" || (state === "transcribing" && !error);
  const title = state === "connecting"
    ? "Connecting to UltraVox"
    : state === "recording"
      ? "Listening"
      : state === "transcribing"
        ? "Transcribing locally"
        : "Review before inserting";

  const waveform = Array.from({ length: 41 }, (_, index) => (
    levels[levels.length - 41 + index] ?? 0
  ));
  const currentLevel = waveform[waveform.length - 1] ?? 0;

  return createPortal(
    <div className="voice-modal-backdrop" data-theme={theme} role="presentation">
      <section
        ref={modalRef}
        className={`voice-modal voice-modal--${state}`}
        role="dialog"
        aria-modal="true"
        aria-busy={isBusy}
        aria-labelledby="voice-modal-title"
        aria-describedby="voice-modal-detail"
      >
        <div className="voice-modal__glow" aria-hidden="true" />
        <header className="voice-modal__header">
          <div className="voice-modal__icon" aria-hidden="true">
            {state === "preview" ? <Sparkles size={18} /> : <Mic size={18} />}
          </div>
          <div>
            <p className="eyebrow">ULTRAVOX VOICE · {terminalLabel.toUpperCase()}</p>
            <h2 id="voice-modal-title">{title}</h2>
          </div>
          <button ref={closeButtonRef} type="button" className="voice-modal__close" onClick={onCancel} aria-label="Cancel voice input">
            <X size={15} />
          </button>
        </header>

        <div className="voice-modal__body">
          {state === "recording" && (
            <div className="voice-listening">
              <ThinkingOrb
                state="listening"
                size={64}
                theme={theme === "white" ? "light" : "dark"}
                aria-hidden="true"
              />
              <div
                className="voice-wave voice-wave--compact"
                role="meter"
                aria-label="Live microphone level"
                aria-valuemin={0}
                aria-valuemax={100}
                aria-valuenow={Math.round(currentLevel * 100)}
                style={{
                  "--voice-level": currentLevel,
                  "--voice-glow": `${12 + currentLevel * 30}px`,
                  "--voice-alpha": 0.08 + currentLevel * 0.22,
                  "--voice-shadow-alpha": currentLevel * 0.2,
                } as CSSProperties}
              >
                <div className="voice-wave__bars" aria-hidden="true">
                  {waveform.map((level, index) => (
                    <span
                      key={index}
                      style={{
                        "--wave-height": `${4 + level * 68}px`,
                        "--wave-opacity": 0.34 + level * 0.66,
                        "--wave-glow": `${3 + level * 9}px`,
                      } as CSSProperties}
                    />
                  ))}
                </div>
                <div className="voice-wave__status">
                  <span>Live input</span>
                  <output>{Math.round(currentLevel * 100)}%</output>
                </div>
              </div>
            </div>
          )}

          {isBusy && (
            <ThinkingOrb
              state={state === "connecting" ? "working" : "solving"}
              size={64}
              theme={theme === "white" ? "light" : "dark"}
              aria-hidden="true"
            />
          )}

          {state === "preview" && (
            <label className="voice-transcript">
              <span>Transcript</span>
              <textarea
                ref={textareaRef}
                value={transcript}
                rows={7}
                spellCheck
                onChange={(event) => onTranscriptChange(event.target.value)}
              />
            </label>
          )}

          <p id="voice-modal-detail" className="voice-modal__detail" role="status" aria-live="polite" aria-atomic="true">
            {state === "connecting" && "Starting the installed UltraVox app and its private local voice service."}
            {state === "recording" && (
              activationSource === "keyboard"
                ? "Speak naturally. The live waveform follows your microphone; press Enter to stop and transcribe."
                : "Speak naturally. The live waveform follows your microphone; press X again to stop and transcribe."
            )}
            {state === "transcribing" && (
              error
                ? "UltraVox status was interrupted. Retry the status check or cancel this recording."
                : "Audio stays on this Mac while UltraVox prepares an editable transcript."
            )}
            {state === "preview" && (
              activationSource === "keyboard"
                ? "Edit anything you need. Press Enter to insert into the selected terminal."
                : "Edit anything you need. Press Enter or X to insert into the selected terminal."
            )}
          </p>
          {error && <p className="voice-modal__error" role="alert">{error}</p>}
        </div>

        <footer className="voice-modal__footer">
          <button type="button" className="voice-modal__cancel" onClick={onCancel}>
            {state === "preview" ? "Discard" : "Cancel"}
            <kbd>{activationSource === "keyboard" ? "Esc" : "Circle"}</kbd>
          </button>
          {state === "transcribing" && error && (
            <button
              type="button"
              className="voice-modal__advance"
              onClick={onAdvance}
            >
              Retry status
              <kbd>{activationSource === "keyboard" ? "Enter" : "X"}</kbd>
            </button>
          )}
          {(state === "recording" || state === "preview") && (
            <button
              type="button"
              className="voice-modal__advance"
              onClick={onAdvance}
              disabled={advancing || (state === "preview" && transcript.trim().length === 0)}
            >
              {state === "recording" ? "Stop & transcribe" : advancing ? "Inserting…" : "Insert in terminal"}
              <kbd>{activationSource === "keyboard" ? "Enter" : "X"}</kbd>
            </button>
          )}
        </footer>
      </section>
    </div>,
    document.body,
  );
}
