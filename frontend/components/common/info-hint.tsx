"use client";

import * as React from "react";
import { Info } from "lucide-react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { cn } from "@/lib/utils";

const GAP = 8;
const EDGE = 8;

function supportsPopover(element: HTMLElement) {
  return typeof (element as HTMLElement & { showPopover?: unknown }).showPopover === "function";
}

/**
 * An ⓘ holding the detail behind a short line of copy. Hover or keyboard focus
 * shows it; a click or tap pins it until Esc or a press elsewhere. The bubble
 * is the trigger's accessible description even while closed, and it renders
 * in the top layer so modal `<dialog>`s and scrolling lists do not clip it.
 */
export function InfoHint({ label, children, className }: {
  /** Accessible name of the trigger, e.g. "About C2C". */
  label?: string;
  children: React.ReactNode;
  className?: string;
}) {
  const { t } = useLocaleText();
  const id = React.useId();
  const trigger = React.useRef<HTMLButtonElement>(null);
  const bubble = React.useRef<HTMLSpanElement>(null);
  const [open, setOpen] = React.useState(false);
  const [position, setPosition] = React.useState<{ top: number; left: number } | null>(null);
  const pinned = React.useRef(false);
  // Only a pinned bubble takes pointer events; a hover or focus preview must
  // never cover the control underneath it.
  const [pinnedOpen, setPinnedOpen] = React.useState(false);
  const pressInside = React.useRef(false);
  const timer = React.useRef<number | undefined>(undefined);

  const schedule = (next: boolean, delay: number) => {
    window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => setOpen(next), delay);
  };
  const close = React.useCallback(() => {
    window.clearTimeout(timer.current);
    pinned.current = false;
    setPinnedOpen(false);
    setOpen(false);
  }, []);

  const place = React.useCallback(() => {
    const anchor = trigger.current?.getBoundingClientRect();
    const box = bubble.current?.getBoundingClientRect();
    if (!anchor || !box) return;
    const left = Math.min(Math.max(anchor.left + anchor.width / 2 - box.width / 2, EDGE), window.innerWidth - box.width - EDGE);
    const below = anchor.bottom + GAP;
    const top = below + box.height > window.innerHeight - EDGE && anchor.top - GAP - box.height >= EDGE
      ? anchor.top - GAP - box.height
      : below;
    setPosition({ top, left: Math.max(left, EDGE) });
  }, []);

  React.useLayoutEffect(() => {
    const element = bubble.current;
    if (!element) return;
    if (supportsPopover(element)) {
      if (!element.hasAttribute("popover")) element.setAttribute("popover", "manual");
      try {
        if (open && !element.matches(":popover-open")) element.showPopover();
        if (!open && element.matches(":popover-open")) element.hidePopover();
      } catch {
        // A detached or already-toggled element needs no change.
      }
    }
    if (open) place();
    else setPosition(null);
  }, [open, place]);

  React.useEffect(() => {
    if (!open) return;
    const onKey = (event: KeyboardEvent) => { if (event.key === "Escape") close(); };
    const onPointer = (event: PointerEvent) => {
      const target = event.target as Node;
      if (!trigger.current?.contains(target) && !bubble.current?.contains(target)) close();
    };
    window.addEventListener("keydown", onKey);
    document.addEventListener("pointerdown", onPointer, true);
    window.addEventListener("scroll", place, true);
    window.addEventListener("resize", place);
    return () => {
      window.removeEventListener("keydown", onKey);
      document.removeEventListener("pointerdown", onPointer, true);
      window.removeEventListener("scroll", place, true);
      window.removeEventListener("resize", place);
    };
  }, [close, open, place]);

  React.useEffect(() => () => window.clearTimeout(timer.current), []);

  const leave = (event: React.PointerEvent) => {
    // A hint opened by keyboard focus stays until focus moves.
    if (event.pointerType === "mouse" && !pinned.current && document.activeElement !== trigger.current) schedule(false, 150);
  };

  return (
    <span className={cn("inline-flex shrink-0 align-middle", className)}>
      <button
        ref={trigger}
        type="button"
        aria-label={label ?? t("common.moreInfo")}
        aria-describedby={id}
        data-open={open || undefined}
        className="-m-2.5 inline-flex size-11 shrink-0 items-center justify-center rounded-full text-muted-foreground transition-colors duration-200 hover:text-accent focus-visible:outline-2 focus-visible:outline-offset-[-8px] focus-visible:outline-ring data-[open]:text-accent"
        onPointerEnter={(event) => { if (event.pointerType === "mouse") schedule(true, 120); }}
        onPointerLeave={leave}
        onFocus={(event) => {
          // Keyboard focus only: a dialog that focuses its first control on
          // open must not cover its own actions with a bubble.
          let keyboard = false;
          try { keyboard = event.currentTarget.matches(":focus-visible"); } catch { /* Older engines skip it. */ }
          if (!keyboard) return;
          window.clearTimeout(timer.current);
          setOpen(true);
        }}
        onBlur={() => {
          if (pressInside.current) { pressInside.current = false; return; }
          close();
        }}
        onClick={() => {
          window.clearTimeout(timer.current);
          if (open && pinned.current) close();
          else { pinned.current = true; setPinnedOpen(true); setOpen(true); }
        }}
        onKeyDown={(event) => {
          // Keep Esc from also dismissing an enclosing dialog.
          if (event.key === "Escape" && open) { event.preventDefault(); event.stopPropagation(); close(); }
        }}
      >
        <Info className="size-3.5" aria-hidden="true" />
      </button>
      <span
        ref={bubble}
        id={id}
        role="tooltip"
        onPointerEnter={() => window.clearTimeout(timer.current)}
        onPointerLeave={leave}
        onPointerDown={() => {
          // Selecting text in the bubble blurs the trigger; keep the bubble open.
          pressInside.current = true;
          window.setTimeout(() => { pressInside.current = false; }, 0);
        }}
        style={position ? { top: position.top, left: position.left } : { top: 0, left: 0, visibility: "hidden" }}
        className={cn(
          "fixed z-[1000] m-0 block w-max max-w-[min(20rem,calc(100vw-1rem))] overflow-visible rounded-xl border border-border bg-card p-3 text-left text-xs font-normal leading-5 text-foreground shadow-lg",
          !open && "hidden",
          !pinnedOpen && "pointer-events-none",
        )}
      >
        {children}
      </span>
    </span>
  );
}
