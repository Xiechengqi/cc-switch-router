"use client";

import * as React from "react";
import { Check, type LucideIcon } from "lucide-react";
import { cn } from "@/lib/utils";

/**
 * One option of a radio group drawn as a card: icon, name, a line of help and
 * the selection mark. Put several inside an element with `role="radiogroup"`.
 */
export function ChoiceCard({
  name,
  value,
  checked,
  disabled = false,
  onChange,
  icon: Icon,
  title,
  description,
  selectedLabel,
}: {
  name: string;
  value: string;
  checked: boolean;
  disabled?: boolean;
  onChange: (value: string) => void;
  icon: LucideIcon;
  title: React.ReactNode;
  description?: React.ReactNode;
  selectedLabel: React.ReactNode;
}) {
  return (
    <label
      className={cn(
        "group relative flex min-h-16 cursor-pointer items-center gap-3 rounded-xl border px-3 py-3 transition-all duration-200 sm:px-4",
        checked
          ? "border-accent bg-accent/5 shadow-[0_4px_14px_rgba(0,82,255,0.15)]"
          : "border-border bg-card hover:border-accent/30 hover:shadow-md",
        disabled && !checked && "cursor-not-allowed opacity-60 hover:border-border hover:shadow-none",
        "focus-within:ring-2 focus-within:ring-accent focus-within:ring-offset-2",
      )}
    >
      <input
        type="radio"
        name={name}
        className="sr-only"
        value={value}
        checked={checked}
        disabled={disabled}
        onChange={() => onChange(value)}
      />
      <span
        className={cn(
          "flex h-9 w-9 shrink-0 items-center justify-center rounded-lg transition-all duration-200",
          checked
            ? "bg-gradient-to-br from-accent to-[rgb(var(--router-accent-secondary))] text-white"
            : "bg-muted text-muted-foreground",
        )}
        aria-hidden
      >
        <Icon className="h-4 w-4" />
      </span>
      <div className="min-w-0 flex-1">
        <div className="text-sm font-medium text-foreground">{title}</div>
        {description ? <div className="text-xs leading-relaxed text-muted-foreground">{description}</div> : null}
      </div>
      {checked ? (
        <span aria-hidden className="inline-flex shrink-0 items-center gap-1.5 rounded-full bg-accent px-2.5 py-1 text-xs font-medium text-white">
          <Check className="h-3 w-3" aria-hidden />
          {selectedLabel}
        </span>
      ) : (
        <span
          className={cn(
            "h-4 w-4 shrink-0 rounded-full border-2 transition-colors duration-200",
            disabled ? "border-border" : "border-muted-foreground/40 group-hover:border-accent",
          )}
          aria-hidden
        />
      )}
    </label>
  );
}
