"use client";

import * as React from "react";
import { cn } from "@/lib/utils";

export type UnderlineTabItem<T extends string> = {
  id: T;
  label: React.ReactNode;
  /** Required when `label` carries more than the tab's name, e.g. a count. */
  ariaLabel?: string;
  title?: string;
};

/**
 * Second-level tabs under a `SegmentedControl`: plain text with an accent
 * underline, so the two levels never look alike. 44px targets; arrow keys,
 * Home and End move the selection like a tablist.
 */
export function UnderlineTabs<T extends string>({ value, onChange, items, ariaLabel, disabled = false, className }: {
  value: T;
  onChange: (next: T) => void;
  items: UnderlineTabItem<T>[];
  ariaLabel: string;
  disabled?: boolean;
  className?: string;
}) {
  const listRef = React.useRef<HTMLDivElement>(null);
  const move = (target: UnderlineTabItem<T> | undefined) => {
    if (!target || disabled) return;
    onChange(target.id);
    listRef.current?.querySelector<HTMLButtonElement>(`[data-tab-id="${CSS.escape(target.id)}"]`)?.focus();
  };
  const index = Math.max(0, items.findIndex((item) => item.id === value));
  return (
    <div
      ref={listRef}
      role="tablist"
      aria-label={ariaLabel}
      className={cn("flex min-w-0 gap-5 overflow-x-auto shadow-[inset_0_-1px_0_rgb(var(--router-border))]", className)}
      onKeyDown={(event) => {
        const next = { ArrowRight: index + 1, ArrowDown: index + 1, ArrowLeft: index - 1, ArrowUp: index - 1, Home: 0, End: items.length - 1 }[event.key];
        if (next === undefined) return;
        event.preventDefault();
        move(items[(next + items.length) % items.length]);
      }}
    >
      {items.map((item) => {
        const selected = item.id === value;
        return (
          <button
            key={item.id}
            type="button"
            role="tab"
            data-tab-id={item.id}
            aria-selected={selected}
            aria-label={item.ariaLabel}
            title={item.title}
            tabIndex={selected && !disabled ? 0 : -1}
            disabled={disabled}
            onClick={() => onChange(item.id)}
            className={cn(
              "inline-flex min-h-11 shrink-0 items-center gap-1.5 whitespace-nowrap border-b-2 px-0.5 text-sm transition-colors duration-200 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring disabled:cursor-not-allowed disabled:opacity-50",
              selected ? "border-accent font-semibold text-foreground" : "border-transparent font-medium text-slate-600 hover:border-slate-300 hover:text-foreground",
            )}
          >
            {item.label}
          </button>
        );
      })}
    </div>
  );
}
