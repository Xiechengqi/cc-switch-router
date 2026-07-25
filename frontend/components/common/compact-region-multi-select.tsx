"use client";

import * as React from "react";
import { ChevronDown, X } from "lucide-react";
import { cn } from "@/lib/utils";

export function CompactRegionMultiSelect({
  values,
  options,
  onChange,
  allLabel,
  moreLabel,
  clearLabel,
  ariaLabel,
  className,
  compact = false,
}: {
  values: string[];
  options: { value: string; label: string }[];
  onChange: (values: string[]) => void;
  allLabel: string;
  moreLabel: (count: number) => string;
  clearLabel: string;
  ariaLabel: string;
  className?: string;
  compact?: boolean;
}) {
  const [open, setOpen] = React.useState(false);
  const [hovered, setHovered] = React.useState(false);
  const rootRef = React.useRef<HTMLDivElement>(null);
  const hasSelection = values.length > 0;
  const showClear = hasSelection && hovered;

  React.useEffect(() => {
    if (!open) return;
    const onPointerDown = (event: PointerEvent) => {
      if (rootRef.current?.contains(event.target as Node)) return;
      setOpen(false);
    };
    window.addEventListener("pointerdown", onPointerDown);
    return () => window.removeEventListener("pointerdown", onPointerDown);
  }, [open]);

  const summary = React.useMemo(() => {
    if (values.length === 0) return allLabel;
    const labels = values.map((value) => options.find((option) => option.value === value)?.label || value);
    if (labels.length === 1) return labels[0];
    if (labels.length === 2) return labels.join(", ");
    return `${labels[0]}, ${labels[1]} ${moreLabel(labels.length - 2)}`;
  }, [allLabel, moreLabel, options, values]);

  const selectAll = () => {
    onChange([]);
  };

  const toggleCountry = (value: string) => {
    const selected = new Set(values);
    if (selected.has(value)) selected.delete(value);
    else selected.add(value);
    onChange(Array.from(selected).sort((left, right) => left.localeCompare(right)));
  };

  return (
    <div ref={rootRef} className={cn("relative", className)}>
      <div
        className={cn(
          "flex w-full items-center transition-colors",
          compact
            ? cn(
                "min-h-6 rounded-md border border-transparent",
                open || hasSelection
                  ? "bg-muted/50 text-foreground"
                  : "text-muted-foreground hover:bg-muted/40 hover:text-foreground",
              )
            : "min-h-9 rounded-lg border bg-white shadow-sm",
        )}
        onMouseEnter={() => setHovered(true)}
        onMouseLeave={() => setHovered(false)}
      >
        <button
          type="button"
          aria-label={ariaLabel}
          aria-expanded={open}
          onClick={() => setOpen((current) => !current)}
          className={cn(
            "flex min-w-0 flex-1 items-center text-left",
            compact ? "gap-1 px-1.5 py-0.5" : "px-3 py-2 text-xs",
          )}
        >
          <span
            className={cn(
              "min-w-0 truncate font-medium",
              compact
                ? cn("text-[11px] font-normal", hasSelection ? "text-foreground" : "text-muted-foreground")
                : "pr-2 text-xs text-foreground",
            )}
          >
            {summary}
          </span>
        </button>
        {showClear ? (
          <button
            type="button"
            onClick={(event) => {
              event.stopPropagation();
              onChange([]);
            }}
            className={cn(
              "inline-flex shrink-0 items-center justify-center rounded-sm text-slate-400 transition-colors hover:bg-slate-100 hover:text-slate-600",
              compact ? "mr-0.5 h-3.5 w-3.5" : "mr-0.5 h-4 w-4",
            )}
            aria-label={clearLabel}
            title={clearLabel}
          >
            <X className={cn(compact ? "h-2.5 w-2.5" : "h-3 w-3")} aria-hidden />
          </button>
        ) : null}
        <button
          type="button"
          aria-hidden
          tabIndex={-1}
          onClick={() => setOpen((current) => !current)}
          className={cn(
            "inline-flex shrink-0 items-center justify-center text-muted-foreground",
            compact ? "px-1 py-0.5" : "px-2 py-2",
          )}
        >
          <ChevronDown
            className={cn(
              "transition-transform",
              compact ? "h-3 w-3 opacity-60" : "h-3.5 w-3.5",
              open && "rotate-180",
            )}
          />
        </button>
      </div>
      {open ? (
        <div
          className={cn(
            "absolute z-50 max-h-64 min-w-full overflow-y-auto rounded-lg border border-border bg-white py-1 text-slate-900 shadow-md",
            compact ? "left-0 top-[calc(100%+2px)] min-w-[10rem]" : "right-0 top-[calc(100%+4px)]",
          )}
        >
          <label className="flex cursor-pointer items-center gap-2 px-3 py-1.5 text-xs text-slate-700 hover:bg-slate-50">
            <input
              type="checkbox"
              checked={values.length === 0}
              onChange={() => selectAll()}
              className="h-3.5 w-3.5 accent-[var(--accent,#0052FF)]"
            />
            <span>{allLabel}</span>
          </label>
          {options.map((option) => (
            <label
              key={option.value}
              className="flex cursor-pointer items-center gap-2 px-3 py-1.5 text-xs text-slate-700 hover:bg-slate-50"
            >
              <input
                type="checkbox"
                checked={values.includes(option.value)}
                onChange={() => {
                  if (values.length === 0) {
                    onChange([option.value]);
                    return;
                  }
                  toggleCountry(option.value);
                }}
                className="h-3.5 w-3.5 accent-[var(--accent,#0052FF)]"
              />
              <span className="min-w-0 truncate">{option.label}</span>
            </label>
          ))}
        </div>
      ) : null}
    </div>
  );
}
