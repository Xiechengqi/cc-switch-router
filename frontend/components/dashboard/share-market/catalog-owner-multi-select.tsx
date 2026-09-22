"use client";

import * as React from "react";
import { ChevronDown, Search, X } from "lucide-react";
import { cn } from "@/lib/utils";

type OwnerOption = { value: string; label: string; rented?: boolean };

export function CatalogOwnerMultiSelect({
  values,
  options,
  onChange,
  allLabel,
  searchLabel,
  emptyLabel,
  moreLabel,
  rentedLabel,
  clearLabel,
  ariaLabel,
  className,
}: {
  values: string[];
  options: OwnerOption[];
  onChange: (values: string[]) => void;
  allLabel: string;
  searchLabel: string;
  emptyLabel: string;
  moreLabel: (count: number) => string;
  rentedLabel: string;
  clearLabel: string;
  ariaLabel: string;
  className?: string;
}) {
  const [open, setOpen] = React.useState(false);
  const [query, setQuery] = React.useState("");
  const rootRef = React.useRef<HTMLDivElement>(null);
  const searchRef = React.useRef<HTMLInputElement>(null);
  const selected = React.useMemo(() => new Set(values), [values]);
  const filtered = React.useMemo(() => {
    const needle = query.trim().toLocaleLowerCase();
    if (!needle) return options;
    return options.filter((option) =>
      option.label.toLocaleLowerCase().includes(needle)
      || option.value.toLocaleLowerCase().includes(needle),
    );
  }, [options, query]);
  const summary = React.useMemo(() => {
    if (!values.length) return allLabel;
    const labels = values.map((value) =>
      options.find((option) => option.value === value)?.label || value,
    );
    if (labels.length === 1) return labels[0];
    if (labels.length === 2) return labels.join(", ");
    return `${labels[0]}, ${labels[1]} ${moreLabel(labels.length - 2)}`;
  }, [allLabel, moreLabel, options, values]);

  React.useEffect(() => {
    if (!open) {
      setQuery("");
      return;
    }
    const timer = window.setTimeout(() => searchRef.current?.focus(), 0);
    const onPointerDown = (event: PointerEvent) => {
      const target = event.target as Node;
      if (rootRef.current?.contains(target)) return;
      setOpen(false);
    };
    window.addEventListener("pointerdown", onPointerDown);
    return () => {
      window.clearTimeout(timer);
      window.removeEventListener("pointerdown", onPointerDown);
    };
  }, [open]);

  const toggle = (value: string) => {
    const next = new Set(selected);
    if (next.has(value)) next.delete(value);
    else next.add(value);
    onChange(
      options
        .map((option) => option.value)
        .filter((item) => next.has(item)),
    );
  };

  return (
    <div ref={rootRef} className={cn("relative", className)}>
      <div className="flex min-h-9 w-full items-center rounded-lg border bg-white shadow-sm">
        <button
          type="button"
          aria-label={ariaLabel}
          aria-expanded={open}
          onClick={() => setOpen((current) => !current)}
          className="flex min-w-0 flex-1 items-center gap-2 px-3 py-2 text-left text-xs"
        >
          <span className="min-w-0 flex-1 truncate font-medium text-foreground">{summary}</span>
          <ChevronDown className={cn("h-3.5 w-3.5 shrink-0 text-muted-foreground transition-transform", open && "rotate-180")} />
        </button>
        {values.length ? (
          <button
            type="button"
            aria-label={clearLabel}
            title={clearLabel}
            className="mr-1 inline-flex h-6 w-6 shrink-0 items-center justify-center rounded text-slate-400 hover:bg-slate-100 hover:text-slate-600"
            onClick={(event) => {
              event.stopPropagation();
              onChange([]);
              setOpen(false);
            }}
          >
            <X className="h-3 w-3" />
          </button>
        ) : null}
      </div>
      {open ? (
        <div className="absolute left-0 top-[calc(100%+4px)] z-[80] min-w-full overflow-hidden rounded-lg border border-border bg-white text-slate-900 shadow-md">
          <label className="flex items-center gap-1.5 border-b border-slate-100 px-2.5 py-2">
            <Search className="h-3.5 w-3.5 shrink-0 text-slate-400" />
            <input
              ref={searchRef}
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              className="min-w-0 flex-1 bg-transparent text-xs outline-none placeholder:text-slate-400"
              placeholder={searchLabel}
              aria-label={searchLabel}
            />
          </label>
          <div className="max-h-56 overflow-y-auto py-1">
            <label className="flex cursor-pointer items-center gap-2 px-3 py-1.5 text-xs text-slate-700 hover:bg-slate-50">
              <input
                type="checkbox"
                checked={values.length === 0}
                onChange={() => onChange([])}
                className="h-3.5 w-3.5 accent-[var(--accent,#0052FF)]"
              />
              <span>{allLabel}</span>
            </label>
            {filtered.length ? filtered.map((option) => (
              <label
                key={option.value}
                className="flex cursor-pointer items-center gap-2 px-3 py-1.5 text-xs text-slate-700 hover:bg-slate-50"
              >
                <input
                  type="checkbox"
                  checked={selected.has(option.value)}
                  onChange={() => toggle(option.value)}
                  className="h-3.5 w-3.5 accent-[var(--accent,#0052FF)]"
                />
                <span className="min-w-0 flex-1 truncate" title={option.label}>{option.label}</span>
                {option.rented ? (
                  <span className="shrink-0 text-[10px] font-medium text-sky-700">{rentedLabel}</span>
                ) : null}
              </label>
            )) : (
              <p className="px-3 py-2 text-xs text-slate-400">{emptyLabel}</p>
            )}
          </div>
        </div>
      ) : null}
    </div>
  );
}
