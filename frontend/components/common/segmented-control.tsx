"use client";

import * as React from "react";
import { cn } from "@/lib/utils";

export type SegmentedControlItem<T extends string> = {
  id: T;
  label: React.ReactNode;
  title?: string;
  ariaLabel?: string;
  /** Applied when the segment is inactive (e.g. status-colored page filters). */
  className?: string;
  /** Unavailable option; arrow keys skip it. */
  disabled?: boolean;
  /** Ids of elements explaining this option, e.g. why it is unavailable. */
  describedBy?: string;
};

export type SegmentedControlProps<T extends string> = {
  value: T;
  onChange: (next: T) => void;
  items: SegmentedControlItem<T>[];
  ariaLabel: string;
  /** Ids of elements describing the whole control. */
  ariaDescribedBy?: string;
  /** `lg` meets the 44px touch target for a page's primary task switch. */
  size?: "sm" | "md" | "lg";
  fullWidth?: boolean;
  disabled?: boolean;
  className?: string;
};

export function SegmentedControl<T extends string>({
  value,
  onChange,
  items,
  ariaLabel,
  ariaDescribedBy,
  size = "sm",
  fullWidth = false,
  disabled = false,
  className,
}: SegmentedControlProps<T>) {
  const listRef = React.useRef<HTMLDivElement>(null);
  const instanceId = React.useId().replaceAll(":", "");

  const focusSegment = (id: T) => {
    const root = listRef.current;
    if (!root) return;
    const button = root.querySelector<HTMLButtonElement>(`[data-segment-id="${CSS.escape(id)}"]`);
    button?.focus();
  };

  const selectByOffset = (offset: number) => {
    if (disabled || items.length === 0) return;
    const index = Math.max(items.findIndex((item) => item.id === value), 0);
    let next: SegmentedControlItem<T> | undefined;
    for (let step = 1; step <= items.length; step += 1) {
      const candidate = items[(index + offset * step + items.length * step) % items.length];
      if (candidate && !candidate.disabled) { next = candidate; break; }
    }
    if (!next || next.id === value) return;
    onChange(next.id);
    focusSegment(next.id);
  };

  return (
    <div
      ref={listRef}
      role="tablist"
      aria-label={ariaLabel}
      aria-describedby={ariaDescribedBy}
      aria-disabled={disabled || undefined}
      className={cn(
        "max-w-full overflow-x-auto rounded-lg bg-slate-100 p-1",
        fullWidth ? "grid w-full gap-0.5 ring-1 ring-inset ring-slate-200/80" : "inline-flex",
        className,
      )}
      style={
        fullWidth && items.length > 0
          ? { gridTemplateColumns: `repeat(${items.length}, minmax(0, 1fr))` }
          : undefined
      }
      onKeyDown={(event) => {
        if (disabled) return;
        if (event.key === "ArrowRight" || event.key === "ArrowDown") {
          event.preventDefault();
          selectByOffset(1);
        } else if (event.key === "ArrowLeft" || event.key === "ArrowUp") {
          event.preventDefault();
          selectByOffset(-1);
        } else if (event.key === "Home") {
          event.preventDefault();
          const first = items.find((item) => !item.disabled);
          if (first) {
            onChange(first.id);
            focusSegment(first.id);
          }
        } else if (event.key === "End") {
          event.preventDefault();
          const last = [...items].reverse().find((item) => !item.disabled);
          if (last) {
            onChange(last.id);
            focusSegment(last.id);
          }
        }
      }}
    >
      {items.map((item) => {
        const selected = item.id === value;
        return (
          <button
            key={item.id}
            type="button"
            role="tab"
            data-segment-id={item.id}
            id={`${instanceId}-${item.id}`}
            title={item.title}
            aria-label={item.ariaLabel || (typeof item.label === "string" ? item.label : undefined)}
            aria-selected={selected}
            aria-describedby={item.describedBy}
            tabIndex={!disabled && selected ? 0 : -1}
            disabled={disabled || item.disabled}
            aria-disabled={disabled || item.disabled || undefined}
            onClick={() => onChange(item.id)}
            className={cn(
              "whitespace-nowrap rounded-md transition-colors active:bg-slate-200 focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary disabled:cursor-not-allowed disabled:opacity-50",
              size === "lg"
                ? "inline-flex h-11 items-center justify-center gap-1.5 px-3 text-sm font-medium"
                : size === "md"
                  ? "inline-flex h-9 items-center justify-center px-3 text-sm font-medium"
                  : "px-2.5 py-1.5 text-[11px]",
              selected
                ? "bg-white font-semibold text-foreground shadow-sm"
                : cn("font-medium hover:text-slate-800", item.className || "text-slate-600"),
            )}
          >
            {item.label}
          </button>
        );
      })}
    </div>
  );
}
