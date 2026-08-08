import { describe, expect, it, vi } from "vitest";
import {
  droppedFileInput,
  insertDroppedFilesIntoActiveTerminal,
} from "./fileDrop";

describe("terminal file drops", () => {
  it("routes a drop only to the highlighted active terminal", async () => {
    const sendTerminalInput = vi.fn().mockResolvedValue(true);

    await expect(insertDroppedFilesIntoActiveTerminal(
      "session-terminal-4",
      ["/tmp/report.pdf"],
      sendTerminalInput,
    )).resolves.toBe(true);

    expect(sendTerminalInput).toHaveBeenCalledOnce();
    expect(sendTerminalInput).toHaveBeenCalledWith(
      "session-terminal-4",
      "'/tmp/report.pdf' ",
    );
  });

  it("quotes multiple paths without submitting the terminal input", () => {
    const input = droppedFileInput([
      "/tmp/my report.pdf",
      "/tmp/it's-ready.txt",
    ]);

    expect(input).toBe("'/tmp/my report.pdf' '/tmp/it'\\''s-ready.txt' ");
    expect(input).not.toMatch(/[\r\n]/);
  });

  it("does nothing without an active terminal or dropped path", async () => {
    const sendTerminalInput = vi.fn().mockResolvedValue(true);

    await expect(insertDroppedFilesIntoActiveTerminal(
      null,
      ["/tmp/report.pdf"],
      sendTerminalInput,
    )).resolves.toBe(false);
    await expect(insertDroppedFilesIntoActiveTerminal(
      "session-terminal-4",
      [],
      sendTerminalInput,
    )).resolves.toBe(false);

    expect(sendTerminalInput).not.toHaveBeenCalled();
  });
});
