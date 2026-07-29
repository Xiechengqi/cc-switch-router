"use client";

import * as React from "react";
import { createPortal } from "react-dom";
import { ChevronDown, X } from "lucide-react";
import { cn } from "@/lib/utils";

type MenuPosition = { top: number; left: number; minWidth: number };

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
  variant = "default",
  columnLabel,
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
  /** Header trigger: single-line `Column ▾` / `Column · value` / `Column · N` */
  variant?: "default" | "header";
  columnLabel?: string;
}) {
  const [open, setOpen] = React.useState(false);
  const [hovered, setHovered] = React.useState(false);
  const [menuPosition, setMenuPosition] = React.useState<MenuPosition | null>(null);
  const rootRef = React.useRef<HTMLDivElement>(null);
  const menuRef = React.useRef<HTMLDivElement>(null);
  const hasSelection = values.length > 0;
  const showClear = hasSelection && hovered;
  const isHeader = variant === "header";
  // Header filters live inside overflow table shells; portal so menus are not clipped.
  const usePortal = isHeader;

  const updateMenuPosition = React.useCallback(() => {
    const root = rootRef.current;
    if (!root) return;
    const rect = root.getBoundingClientRect();
    const minWidth = Math.max(rect.width, isHeader || compact ? 160 : rect.width);
    const estimatedHeight = Math.min(256, 40 + options.length * 28);
    const spaceBelow = window.innerHeight - rect.bottom - 8;
    const openUp = spaceBelow < estimatedHeight && rect.top > spaceBelow;
    const top = openUp ? Math.max(8, rect.top - estimatedHeight - 2) : rect.bottom + 2;
    const left = Math.min(rect.left, Math.max(8, window.innerWidth - minWidth - 8));
    setMenuPosition({ top, left, minWidth });
  }, [compact, isHeader, options.length]);

  React.useEffect(() => {
    if (!open) {
      setMenuPosition(null);
      return;
    }
    updateMenuPosition();
    if (!usePortal) return;
    const onReposition = () => updateMenuPosition();
    window.addEventListener("resize", onReposition);
    window.addEventListener("scroll", onReposition, true);
    return () => {
      window.removeEventListener("resize", onReposition);
      window.removeEventListener("scroll", onReposition, true);
    };
  }, [open, updateMenuPosition, usePortal]);

  React.useEffect(() => {
    if (!open) return;
    const onPointerDown = (event: PointerEvent) => {
      const target = event.target as Node;
      if (rootRef.current?.contains(target) || menuRef.current?.contains(target)) return;
      setOpen(false);
    };
    window.addEventListener("pointerdown", onPointerDown);
    return () => window.removeEventListener("pointerdown", onPointerDown);
  }, [open]);

  const summary = React.useMemo(() => {
    const labels = values.map((value) => options.find((option) => option.value === value)?.label || value);
    if (isHeader) {
      const title = columnLabel || allLabel;
      if (values.length === 0) return title;
      if (values.length === 1) return `${title} · ${labels[0]}`;
      return `${title} · ${values.length}`;
    }
    if (values.length === 0) return allLabel;
    if (labels.length === 1) return labels[0];
    if (labels.length === 2) return labels.join(", ");
    return `${labels[0]}, ${labels[1]} ${moreLabel(labels.length - 2)}`;
  }, [allLabel, columnLabel, isHeader, moreLabel, options, values]);

  const selectAll = () => {
    onChange([]);
  };

  const toggleCountry = (value: string) => {
    const selected = new Set(values);
    if (selected.has(value)) selected.delete(value);
    else selected.add(value);
    onChange(Array.from(selected).sort((left, right) => left.localeCompare(right)));
  };

  const menu = open ? (
    <div
      ref={menuRef}
      className={cn(
        "z-[80] max-h-64 overflow-y-auto rounded-lg border border-border bg-white py-1 text-slate-900 shadow-md",
        usePortal ? "fixed" : "absolute",
        !usePortal && (isHeader || compact ? "left-0 top-[calc(100%+2px)] min-w-[10rem]" : "right-0 top-[calc(100%+4px)] min-w-full"),
      )}
      style={
        usePortal && menuPosition
          ? { top: menuPosition.top, left: menuPosition.left, minWidth: menuPosition.minWidth }
          : undefined
      }
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
  ) : null;

  return (
    <div ref={rootRef} className={cn("relative", className)}>
      <div
        className={cn(
          "flex w-full items-center transition-colors",
          isHeader
            ? cn(
                "min-h-7 rounded-md",
                open || hasSelection
                  ? "bg-muted/45 text-foreground"
                  : "text-muted-foreground hover:bg-muted/35 hover:text-foreground",
              )
            : compact
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
            isHeader ? "gap-0.5 px-1 py-0.5" : compact ? "gap-1 px-1.5 py-0.5" : "px-3 py-2 text-xs",
          )}
        >
          <span
            className={cn(
              "min-w-0 truncate font-medium",
              isHeader
                ? cn("text-xs", hasSelection ? "font-medium text-foreground" : "font-medium text-muted-foreground")
                : compact
                  ? cn("text-[11px] font-normal", hasSelection ? "text-foreground" : "text-muted-foreground")
                  : "pr-2 text-xs text-foreground",
            )}
          >
            {summary}
          </span>
          {isHeader ? (
            <ChevronDown
              className={cn("h-3 w-3 shrink-0 opacity-50 transition-transform", open && "rotate-180")}
              aria-hidden
            />
          ) : null}
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
              isHeader || compact ? "mr-0.5 h-3.5 w-3.5" : "mr-0.5 h-4 w-4",
            )}
            aria-label={clearLabel}
            title={clearLabel}
          >
            <X className={cn(isHeader || compact ? "h-2.5 w-2.5" : "h-3 w-3")} aria-hidden />
          </button>
        ) : null}
        {!isHeader ? (
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
        ) : null}
      </div>
      {usePortal && typeof document !== "undefined" && menu
        ? createPortal(menu, document.body)
        : menu}
    </div>
  );
}
