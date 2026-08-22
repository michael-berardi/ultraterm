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

function stubStorage(value: string | null, current = false): void {
  vi.stubGlobal("window", {
    localStorage: {
      getItem: vi.fn((key: string) => key.endsWith("-v2") === current ? value : null),
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
      { slot: 3, launchProfile: "fast-worker" },
      { slot: 1, launchProfile: "reviewer" },
      { slot: 9, launchProfile: "reviewer" },
      { slot: 0, launchProfile: null },
      { slot: 2, launchProfile: null },
      { slot: 2, launchProfile: "fast-worker" },
      { slot: 4, launchProfile: "quiet-planner" },
      { slot: 5, launchProfile: "my-custom-profile" },
    ]));

    expect(readTerminalLaunchRows(8)).toEqual([
      { slot: 1, launchProfile: "reviewer" },
      { slot: 2, launchProfile: "fast-worker" },
      { slot: 3, launchProfile: "fast-worker" },
      { slot: 4, launchProfile: "quiet-planner" },
      { slot: 5, launchProfile: "my-custom-profile" },
    ]);
  });

  it("migrates legacy slot-only arrays to Default OMP rows", () => {
    stubStorage(JSON.stringify([3, 1, 8, 0, 9, 2, 2]));

    expect(readTerminalLaunchRows(8)).toEqual([
      { slot: 1, launchProfile: null },
      { slot: 2, launchProfile: null },
      { slot: 3, launchProfile: null },
      { slot: 8, launchProfile: null },
    ]);
  });

  it("migrates the legacy \"default\" profile id to null", () => {
    stubStorage(JSON.stringify([
      { slot: 1, launchProfile: "default" },
      { slot: 2, launchProfile: "other" },
    ]));

    expect(readTerminalLaunchRows(8)).toEqual([
      { slot: 1, launchProfile: null },
      { slot: 2, launchProfile: "other" },
    ]);
  });

  it("preserves a real profile named default in versioned rows", () => {
    stubStorage(JSON.stringify([{ slot: 1, launchProfile: "default" }]), true);

    expect(readTerminalLaunchRows(8)).toEqual([
      { slot: 1, launchProfile: "default" },
    ]);
  });

  it("preserves arbitrary profile names and maps empty or non-string values to null", () => {
    stubStorage(JSON.stringify([
      { slot: 1, launchProfile: "any-name-the-user-made" },
      { slot: 2, launchProfile: "" },
      { slot: 3, launchProfile: 42 },
    ]));

    expect(readTerminalLaunchRows(8)).toEqual([
      { slot: 1, launchProfile: "any-name-the-user-made" },
      { slot: 2, launchProfile: null },
      { slot: 3, launchProfile: null },
    ]);
  });

  it("returns null when nothing is saved", () => {
    stubStorage(null);

    expect(readTerminalLaunchRows(8)).toBeNull();
  });
});

describe("readLastLaunchProfile", () => {
  it("preserves an arbitrary stored profile name", () => {
    stubStorage("my-custom-profile");

    expect(readLastLaunchProfile()).toBe("my-custom-profile");
  });

  it("migrates the legacy \"default\" id to null", () => {
    stubStorage("default");

    expect(readLastLaunchProfile()).toBeNull();
  });

  it("preserves a real profile named default in versioned storage", () => {
    stubStorage("default", true);

    expect(readLastLaunchProfile()).toBe("default");
  });

  it("preserves an explicit versioned Default OMP selection", () => {
    stubStorage("@default", true);

    expect(readLastLaunchProfile()).toBeNull();
  });

  it("returns null for missing or empty values", () => {
    stubStorage(null);

    expect(readLastLaunchProfile()).toBeNull();

    stubStorage("");

    expect(readLastLaunchProfile()).toBeNull();
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
