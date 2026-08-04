import { useEffect, useRef, useState } from "react";

export const PS4_CONTROL_MAP = [
  { control: "L1 / R1", action: "Previous / next terminal" },
  { control: "D-pad left / right", action: "Previous / next terminal" },
  { control: "D-pad up / down", action: "Scroll terminal output" },
  { control: "Left stick up / down", action: "Scroll terminal output" },
  { control: "L2 / R2", action: "Page up / page down" },
  { control: "X", action: "Record, preview, then enter voice text" },
  { control: "Square", action: "Focus / restore terminal · hold to close" },
  { control: "Triangle", action: "Start a new OMP session" },
  { control: "Circle", action: "Clear input / cancel voice / Escape · hold to close" },
  { control: "Select / Share", action: "Show recent OMP sessions (/resume)" },
  { control: "Options / touchpad", action: "Show controller controls" },
] as const;

interface Ps4ControllerOptions {
  activeId: string | null;
  sessionIds: string[];
  onSelect: (id: string) => void;
  onScrollLines: (id: string, lines: number) => void;
  onScrollPages: (id: string, pages: number) => void;
  onToggleMaximize: (id: string) => void;
  onClose: (id: string) => void;
  onNewSession: (id: string) => void;
  onCross: () => void;
  onCircle: (id: string) => void;
  onResume: (id: string) => void;
  onOpenControls: () => void;
}


function isPs4Controller(gamepad: Gamepad): boolean {
  return /dualshock|wireless controller|sony|ps4/i.test(gamepad.id);
}

export function usePs4Controller(options: Ps4ControllerOptions) {
  const optionsRef = useRef(options);
  const previousButtons = useRef<boolean[]>([]);
  const squarePressStarted = useRef<number | null>(null);
  const squareHoldFired = useRef(false);
  const circlePressStarted = useRef<number | null>(null);
  const circleHoldFired = useRef(false);
  const lastVerticalScroll = useRef(0);
  const [controllerName, setControllerName] = useState<string | null>(null);
  optionsRef.current = options;

  useEffect(() => {
    let animationFrame = 0;
    let discoveryTimer = 0;
    let connectedIndex: number | null = null;

    const selectRelative = (direction: -1 | 1) => {
      const { activeId, sessionIds, onSelect } = optionsRef.current;
      if (sessionIds.length === 0) return;
      const currentIndex = Math.max(0, sessionIds.indexOf(activeId ?? ""));
      const nextIndex = (currentIndex + direction + sessionIds.length) % sessionIds.length;
      onSelect(sessionIds[nextIndex]);
    };

    const runPressedAction = (index: number) => {
      const current = optionsRef.current;
      const activeId = current.activeId;
      if (index === 4 || index === 14) return selectRelative(-1);
      if (index === 5 || index === 15) return selectRelative(1);
      if (index === 8) {
        if (activeId) current.onResume(activeId);
        return;
      }
      if (index === 9 || index === 17) return current.onOpenControls();
      if (index === 0) return current.onCross();
      if (!activeId) return;
      if (index === 12) current.onScrollLines(activeId, -8);
      else if (index === 13) current.onScrollLines(activeId, 8);
      else if (index === 6) current.onScrollPages(activeId, -1);
      else if (index === 7) current.onScrollPages(activeId, 1);
      else if (index === 2) current.onToggleMaximize(activeId);
      else if (index === 3) current.onNewSession(activeId);
    };

    const handleSquare = (pressed: boolean, wasPressed: boolean, timestamp: number) => {
      if (pressed && !wasPressed) {
        squarePressStarted.current = timestamp;
        squareHoldFired.current = false;
        return;
      }
      if (
        pressed
        && !squareHoldFired.current
        && squarePressStarted.current !== null
        && timestamp - squarePressStarted.current >= 700
      ) {
        const activeId = optionsRef.current.activeId;
        if (activeId) optionsRef.current.onClose(activeId);
        squareHoldFired.current = true;
        return;
      }
      if (!pressed && wasPressed) {
        const activeId = optionsRef.current.activeId;
        if (!squareHoldFired.current && activeId) {
          optionsRef.current.onToggleMaximize(activeId);
        }
        squarePressStarted.current = null;
        squareHoldFired.current = false;
      }
    };

    const handleCircle = (pressed: boolean, wasPressed: boolean, timestamp: number) => {
      if (pressed && !wasPressed) {
        circlePressStarted.current = timestamp;
        circleHoldFired.current = false;
        return;
      }
      if (
        pressed
        && !circleHoldFired.current
        && circlePressStarted.current !== null
        && timestamp - circlePressStarted.current >= 700
      ) {
        const activeId = optionsRef.current.activeId;
        if (activeId) optionsRef.current.onClose(activeId);
        circleHoldFired.current = true;
        return;
      }
      if (!pressed && wasPressed) {
        const activeId = optionsRef.current.activeId;
        if (!circleHoldFired.current && activeId) {
          optionsRef.current.onCircle(activeId);
        }
        circlePressStarted.current = null;
        circleHoldFired.current = false;
      }
    };

    const poll = (timestamp: number) => {
      const gamepads = navigator.getGamepads?.() ?? [];
      const controller = Array.from(gamepads).find(
        (gamepad): gamepad is Gamepad => Boolean(gamepad && isPs4Controller(gamepad)),
      ) ?? null;

      if (!controller) {
        if (connectedIndex !== null) {
          connectedIndex = null;
          previousButtons.current = [];
          squarePressStarted.current = null;
          squareHoldFired.current = false;
          circlePressStarted.current = null;
          circleHoldFired.current = false;
          setControllerName(null);
        }
        animationFrame = 0;
        discoveryTimer = window.setTimeout(() => {
          discoveryTimer = 0;
          animationFrame = requestAnimationFrame(poll);
        }, 1_000);
        return;
      }

      if (connectedIndex !== controller.index) {
        connectedIndex = controller.index;
        previousButtons.current = controller.buttons.map((button) => button.pressed);
        setControllerName(controller.id || "PS4 Controller");
      }

      const hasFocus = document.hasFocus();
      controller.buttons.forEach((button, index) => {
        const pressed = button.pressed || button.value > 0.65;
        const wasPressed = previousButtons.current[index] ?? false;

        if (index === 2) {
          if (hasFocus) handleSquare(pressed, wasPressed, timestamp);
        } else if (index === 1) {
          if (hasFocus) handleCircle(pressed, wasPressed, timestamp);
        } else if (hasFocus && pressed && !wasPressed) {
          runPressedAction(index);
        }
        previousButtons.current[index] = pressed;
      });

      const verticalAxis = controller.axes[1] ?? 0;
      if (hasFocus && Math.abs(verticalAxis) > 0.58 && timestamp - lastVerticalScroll.current > 90) {
        const activeId = optionsRef.current.activeId;
        if (activeId) optionsRef.current.onScrollLines(activeId, Math.sign(verticalAxis) * 3);
        lastVerticalScroll.current = timestamp;
      }

      animationFrame = requestAnimationFrame(poll);
    };

    animationFrame = requestAnimationFrame(poll);
    return () => {
      if (animationFrame) cancelAnimationFrame(animationFrame);
      if (discoveryTimer) window.clearTimeout(discoveryTimer);
    };
  }, []);

  return {
    connected: controllerName !== null,
    controllerName,
  };
}
