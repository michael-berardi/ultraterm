import { describe, expect, it } from "vitest";
import { getVoiceModalAction } from "./VoicePreviewModal";

describe("getVoiceModalAction", () => {
  it("advances on Enter in preview for both activation sources", () => {
    expect(
      getVoiceModalAction("preview", "controller", { key: "Enter", repeat: false }),
    ).toBe("advance");
    expect(
      getVoiceModalAction("preview", "keyboard", { key: "Enter", repeat: false }),
    ).toBe("advance");
  });

  it("advances on Enter for keyboard source in non-connecting states", () => {
    expect(
      getVoiceModalAction("recording", "keyboard", { key: "Enter", repeat: false }),
    ).toBe("advance");
    expect(
      getVoiceModalAction("transcribing", "keyboard", { key: "Enter", repeat: false }),
    ).toBe("advance");
  });

  it("does not advance on Enter while connecting", () => {
    expect(
      getVoiceModalAction("connecting", "keyboard", { key: "Enter", repeat: false }),
    ).toBeNull();
  });

  it("does not advance on Enter for controller source outside preview", () => {
    expect(
      getVoiceModalAction("recording", "controller", { key: "Enter", repeat: false }),
    ).toBeNull();
    expect(
      getVoiceModalAction("transcribing", "controller", { key: "Enter", repeat: false }),
    ).toBeNull();
  });

  it("ignores repeated Enter presses", () => {
    expect(
      getVoiceModalAction("preview", "keyboard", { key: "Enter", repeat: true }),
    ).toBeNull();
  });

  it("cancels on Escape", () => {
    expect(
      getVoiceModalAction("recording", "controller", { key: "Escape", repeat: false }),
    ).toBe("cancel");
  });
});
