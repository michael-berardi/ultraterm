import { describe, expect, it, vi } from "vitest";
import {
  appendCappedOutput,
  cleanupOrphanTmuxSlots,
  defaultTerminalSlots,
  ompCommandInput,
  readLastLaunchProfile,
  readTerminalLaunchRows,
  reconnectTerminalSlot,
} from "./useTerminalWorkspace";

describe("appendCappedOutput", () => {
  it("retains only the newest bytes when buffered output exceeds its cap", () => {
    const buffer = { bytes: 0, chunks: [] as Uint8Array[] };

    appendCappedOutput(buffer, new Uint8Array([1, 2, 3]), 5);
    appendCappedOutput(buffer, new Uint8Array([4, 5, 6, 7]), 5);

    expect(buffer.bytes).toBe(5);
    expect(Array.from(buffer.chunks[0])).toEqual([3]);
    expect(Array.from(buffer.chunks[1])).toEqual([4, 5, 6, 7]);
  });

  it("crops a single oversized output chunk to the newest bytes", () => {
    const buffer = { bytes: 0, chunks: [] as Uint8Array[] };

    appendCappedOutput(buffer, new Uint8Array([1, 2, 3, 4, 5, 6]), 3);

    expect(buffer.bytes).toBe(3);
    expect(Array.from(buffer.chunks[0])).toEqual([4, 5, 6]);
  });
});

function stubStorage(value: string | null): void {
  vi.stubGlobal("window", {
    localStorage: {
      getItem: vi.fn().mockReturnValue(value),
    },
  });
}

describe("defaultTerminalSlots", () => {
  it("returns sequential slots starting at 1 up to the target count", () => {
    expect(defaultTerminalSlots(3, 8)).toEqual([1, 2, 3]);
  });

  it("caps the result at the maximum slot count", () => {
    expect(defaultTerminalSlots(10, 8)).toEqual([1, 2, 3, 4, 5, 6, 7, 8]);
  });

  it("returns an empty array when the target count is zero", () => {
    expect(defaultTerminalSlots(0, 8)).toEqual([]);
  });
});

describe("readTerminalLaunchRows", () => {
  it("reads stored slot+profile rows and clamps them to valid range", () => {
    stubStorage(JSON.stringify([
      { slot: 3, launchProfile: "kimi-k3" },
      { slot: 1, launchProfile: "gpt-only" },
      { slot: 9, launchProfile: "gpt-only" },
      { slot: 0, launchProfile: "default" },
      { slot: 2, launchProfile: "default" },
      { slot: 2, launchProfile: "kimi-k3" },
      { slot: 4, launchProfile: "deepseek-v4-flash" },
    ]));

    expect(readTerminalLaunchRows(8)).toEqual([
      { slot: 1, launchProfile: "gpt-only" },
      { slot: 2, launchProfile: "kimi-k3" },
      { slot: 3, launchProfile: "kimi-k3" },
      { slot: 4, launchProfile: "deepseek-v4-flash" },
    ]);
  });

  it("migrates legacy slot-only arrays to default-profile rows", () => {
    stubStorage(JSON.stringify([3, 1, 8, 0, 9, 2, 2]));

    expect(readTerminalLaunchRows(8)).toEqual([
      { slot: 1, launchProfile: "default" },
      { slot: 2, launchProfile: "default" },
      { slot: 3, launchProfile: "default" },
      { slot: 8, launchProfile: "default" },
    ]);
  });

  it("falls back to the default profile for unknown profile ids", () => {
    stubStorage(JSON.stringify([{ slot: 1, launchProfile: "not-a-profile" }]));

    expect(readTerminalLaunchRows(8)).toEqual([
      { slot: 1, launchProfile: "default" },
    ]);
  });

  it("returns null when nothing is saved", () => {
    stubStorage(null);

    expect(readTerminalLaunchRows(8)).toBeNull();
  });
});

describe("readLastLaunchProfile", () => {
  it("returns the stored profile when it is a known id", () => {
    stubStorage("deepseek-v4-flash");

    expect(readLastLaunchProfile()).toBe("deepseek-v4-flash");
  });

  it("falls back to default for missing or unknown values", () => {
    stubStorage("bogus");

    expect(readLastLaunchProfile()).toBe("default");
  });
});

describe("reconnectTerminalSlot", () => {
  it("detaches and replaces only the requested terminal slot", async () => {
    const operations: string[] = [];
    const detachClient = vi.fn(async (id: string) => {
      operations.push(`detach:${id}`);
    });
    const forgetClient = vi.fn((id: string) => {
      operations.push(`forget:${id}`);
    });
    const launch = vi.fn(async (slot: number) => {
      operations.push(`launch:${slot}`);
      return { slot };
    });

    await expect(reconnectTerminalSlot(
      "session-2",
      2,
      detachClient,
      forgetClient,
      launch,
    )).resolves.toEqual({ slot: 2 });

    expect(operations).toEqual(["detach:session-2", "forget:session-2", "launch:2"]);
    expect(detachClient).toHaveBeenCalledTimes(1);
    expect(launch).toHaveBeenCalledWith(2);
  });

  it("does not launch until the old client has fully detached", async () => {
    let finishDetach: (() => void) | undefined;
    const detachClient = vi.fn(() => new Promise<void>((resolve) => {
      finishDetach = resolve;
    }));
    const launch = vi.fn(async () => undefined);

    const reconnecting = reconnectTerminalSlot(
      "stale-session",
      3,
      detachClient,
      vi.fn(),
      launch,
    );
    await Promise.resolve();
    expect(launch).not.toHaveBeenCalled();

    finishDetach?.();
    await reconnecting;
    expect(launch).toHaveBeenCalledWith(3);
  });
});

describe("ompCommandInput", () => {
  it("clears pending prompt input before submitting the selected command", () => {
    expect(ompCommandInput("/new")).toBe("\u0015/new\r");
    expect(ompCommandInput("/resume")).toBe("\u0015/resume\r");
    expect(ompCommandInput("/exit")).toBe("\u0015/exit\r");
  });
});

describe("cleanupOrphanTmuxSlots", () => {
  it("removes persistent tmux slots that are not in the intended set", async () => {
    const removedSlots: number[] = [];
    const listTmuxSlots = vi.fn().mockResolvedValue([1, 2, 3, 4, 6]);
    const removeTmuxSlot = vi.fn().mockImplementation(async (slot: number) => {
      removedSlots.push(slot);
    });

    await cleanupOrphanTmuxSlots([1, 2, 3], listTmuxSlots, removeTmuxSlot);

    expect(listTmuxSlots).toHaveBeenCalledTimes(1);
    expect(removeTmuxSlot).toHaveBeenCalledTimes(2);
    expect(removedSlots).toEqual([4, 6]);
  });

  it("keeps all tracked slots and removes nothing when tmux matches intended", async () => {
    const listTmuxSlots = vi.fn().mockResolvedValue([1, 2, 3]);
    const removeTmuxSlot = vi.fn();

    await cleanupOrphanTmuxSlots([1, 2, 3], listTmuxSlots, removeTmuxSlot);

    expect(removeTmuxSlot).not.toHaveBeenCalled();
  });

  it("removes every persistent slot when intended set is empty", async () => {
    const removedSlots: number[] = [];
    const listTmuxSlots = vi.fn().mockResolvedValue([1, 2, 3]);
    const removeTmuxSlot = vi.fn().mockImplementation(async (slot: number) => {
      removedSlots.push(slot);
    });

    await cleanupOrphanTmuxSlots([], listTmuxSlots, removeTmuxSlot);

    expect(removedSlots).toEqual([1, 2, 3]);
  });
});
