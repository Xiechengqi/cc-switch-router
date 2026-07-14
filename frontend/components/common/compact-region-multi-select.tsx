"use client";

import * as React from "react";
import { ChevronDown } from "lucide-react";
import { cn } from "@/lib/utils";

export function CompactRegionMultiSelect({
  values,
  options,
  onChange,
  allLabel,
  moreLabel,
  ariaLabel,
  className,
}: {
  values: string[];
  options: { value: string; label: string }[];
  onChange: (values: string[]) => void;
  allLabel: string;
  moreLabel: (count: number) => string;
  ariaLabel: string;
  className?: string;
}) {
  const [open, setOpen] = React.useState(false);
  const rootRef = React.useRef<HTMLDivElement>(null);

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
      <button
        type="button"
        aria-label={ariaLabel}
        aria-expanded={open}
        onClick={() => setOpen((current) => !current)}
        className="flex min-h-9 w-full items-center justify-between gap-2 rounded-lg border bg-white px-3 text-xs shadow-sm"
      >
        <span className="min-w-0 truncate pr-2 text-xs font-medium text-foreground">{summary}</span>
        <ChevronDown className={cn("h-3.5 w-3.5 shrink-0 text-muted-foreground transition-transform", open && "rotate-180")} />
      </button>
      {open ? (
        <div className="absolute right-0 top-[calc(100%+4px)] z-50 max-h-64 min-w-full overflow-y-auto rounded-lg border bg-white py-1 shadow-lg">
          <label className="flex cursor-pointer items-center gap-2 px-3 py-2 text-xs hover:bg-slate-50">
            <input
              type="checkbox"
              checked={values.length === 0}
              onChange={() => selectAll()}
            />
            <span>{allLabel}</span>
          </label>
          {options.map((option) => (
            <label key={option.value} className="flex cursor-pointer items-center gap-2 px-3 py-2 text-xs hover:bg-slate-50">
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
              />
              <span>{option.label}</span>
            </label>
          ))}
        </div>
      ) : null}
    </div>
  );
}
