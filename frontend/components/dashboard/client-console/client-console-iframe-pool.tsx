"use client";

import * as React from "react";
import {
  type ConsoleWindow,
  type NormalRect,
  useClientConsole,
} from "@/components/dashboard/client-console/client-console-manager";

function offScreenStyle(window: ConsoleWindow): React.CSSProperties {
  return {
    position: "fixed",
    left: -window.normalRect.width - 100,
    top: 0,
    width: window.normalRect.width,
    height: Math.max(200, window.normalRect.height - 88),
    zIndex: -1,
    opacity: 0,
    pointerEvents: "none",
  };
}

function PositionedIframe({
  consoleWindow,
  targetEl,
  minimized,
}: {
  consoleWindow: ConsoleWindow;
  targetEl: HTMLElement | null;
  minimized: boolean;
}) {
  const [style, setStyle] = React.useState<React.CSSProperties>(() =>
    minimized || !targetEl ? offScreenStyle(consoleWindow) : { position: "fixed", zIndex: consoleWindow.zIndex - 1 },
  );

  React.useLayoutEffect(() => {
    if (!consoleWindow.activated) return;

    function update() {
      if (minimized || !targetEl) {
        setStyle(offScreenStyle(consoleWindow));
        return;
      }
      const rect = targetEl.getBoundingClientRect();
      setStyle({
        position: "fixed",
        top: rect.top,
        left: rect.left,
        width: rect.width,
        height: rect.height,
        zIndex: consoleWindow.zIndex - 1,
        pointerEvents: "auto",
      });
    }

    update();
    if (!targetEl || minimized) return;

    const observer = new ResizeObserver(update);
    observer.observe(targetEl);
    globalThis.window.addEventListener("resize", update);
    globalThis.window.addEventListener("scroll", update, true);
    return () => {
      observer.disconnect();
      globalThis.window.removeEventListener("resize", update);
      globalThis.window.removeEventListener("scroll", update, true);
    };
  }, [consoleWindow, minimized, targetEl]);

  if (!consoleWindow.activated) return null;

  return (
    <div style={style} data-console-iframe={consoleWindow.id}>
      <iframe
        src={consoleWindow.url}
        title={consoleWindow.title}
        className="h-full w-full rounded-xl border-0 bg-white"
        allow="clipboard-read; clipboard-write"
      />
    </div>
  );
}

export function ClientConsoleIframePool() {
  const { windows, getIframeTarget } = useClientConsole();

  return (
    <>
      {windows.map((consoleWindow) => (
        <PositionedIframe
          key={consoleWindow.id}
          consoleWindow={consoleWindow}
          targetEl={getIframeTarget(consoleWindow.id)}
          minimized={consoleWindow.state === "minimized"}
        />
      ))}
    </>
  );
}

export function ClientConsoleIframeSlot({
  windowId,
  className,
}: {
  windowId: string;
  className?: string;
}) {
  const { registerIframeTarget } = useClientConsole();
  const ref = React.useCallback(
    (node: HTMLDivElement | null) => {
      registerIframeTarget(windowId, node);
    },
    [registerIframeTarget, windowId],
  );

  return <div ref={ref} className={className} data-console-slot={windowId} />;
}
