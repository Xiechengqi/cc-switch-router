"use client";

import { Crown, X } from "lucide-react";
import * as React from "react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { splitEmails } from "./share-edit-draft";

export function FieldGroup({
  label,
  hint,
  invalid,
  children,
}: {
  label: string;
  hint?: React.ReactNode;
  invalid?: boolean;
  children: React.ReactNode;
}) {
  const { t } = useLocaleText();
  return (
    <div className="grid gap-1.5 text-sm">
      <span className="mono-label text-muted-foreground">{label}</span>
      {children}
      {hint || invalid ? (
        <span className={`text-xs ${invalid ? "text-red-600" : "text-muted-foreground"}`}>
          {invalid ? t("dashboard.fieldInvalid") : null}
          {hint && !invalid ? hint : null}
        </span>
      ) : null}
    </div>
  );
}

export function EmailTagsField({
  value,
  onChange,
  disabled,
  placeholder,
  onPromote,
  promotableEmails,
  promoteLabel,
}: {
  value: string[];
  onChange: (next: string[]) => void;
  disabled?: boolean;
  placeholder?: string;
  onPromote?: (email: string) => void;
  promotableEmails?: string[];
  promoteLabel?: string;
}) {
  const [draft, setDraft] = React.useState("");
  const promotableSet = React.useMemo(() => new Set(promotableEmails ?? []), [promotableEmails]);
  const commit = (raw: string) => {
    const parts = splitEmails(raw);
    setDraft("");
    if (!parts.length) return;
    const next = [...value];
    for (const part of parts) {
      if (!next.includes(part)) next.push(part);
    }
    if (next.length !== value.length) onChange(next);
  };
  const removeAt = (idx: number) => onChange(value.filter((_, i) => i !== idx));
  return (
    <div
      className={`flex min-h-10 w-full flex-wrap items-center gap-1.5 rounded-lg border border-slate-200 bg-white px-2 py-1.5 text-sm transition-colors focus-within:border-primary/50 ${disabled ? "cursor-not-allowed opacity-60" : ""}`}
    >
      {value.map((email, idx) => {
        const canPromote = !disabled && Boolean(onPromote) && promotableSet.has(email);
        return (
          <span
            key={email}
            className="inline-flex max-w-full items-center gap-1.5 rounded-full border border-primary/20 bg-primary/10 px-2.5 py-1 text-xs font-medium text-primary"
          >
            <span className="min-w-0 truncate">{email}</span>
            {canPromote ? (
              <button
                type="button"
                aria-label={`${promoteLabel ?? "Set as owner"}: ${email}`}
                title={promoteLabel ?? "Set as owner"}
                className="inline-flex h-4 w-4 items-center justify-center rounded-full bg-amber-100/70 text-amber-700 transition-colors hover:bg-amber-200/80"
                onClick={() => onPromote?.(email)}
              >
                <Crown className="h-3 w-3" />
              </button>
            ) : null}
            {disabled ? null : (
              <button
                type="button"
                aria-label={`remove ${email}`}
                className="inline-flex h-4 w-4 items-center justify-center rounded-full bg-primary/15 transition-colors hover:bg-primary/25"
                onClick={() => removeAt(idx)}
              >
                <X className="h-3 w-3" />
              </button>
            )}
          </span>
        );
      })}
      <input
        value={draft}
        disabled={disabled}
        className="h-7 min-w-[10rem] flex-1 bg-transparent text-slate-900 placeholder:text-muted-foreground focus:outline-none disabled:cursor-not-allowed"
        placeholder={value.length ? "" : placeholder}
        onChange={(event) => setDraft(event.target.value)}
        onKeyDown={(event) => {
          if (event.key === "Enter" || event.key === ",") {
            event.preventDefault();
            commit(draft);
          } else if (event.key === "Backspace" && draft === "" && value.length) {
            event.preventDefault();
            removeAt(value.length - 1);
          }
        }}
        onBlur={() => commit(draft)}
        onPaste={(event) => {
          const text = event.clipboardData.getData("text");
          if (/[\s,;]/.test(text)) {
            event.preventDefault();
            commit(text);
          }
        }}
      />
    </div>
  );
}

export function MarketEmailChip({
  label,
  onRemove,
}: {
  label: string;
  onRemove?: () => void;
}) {
  return (
    <span className="inline-flex items-center gap-1.5 rounded-full border border-primary/20 bg-primary/10 px-2.5 py-1 text-xs font-medium text-primary">
      {label}
      {onRemove ? (
        <button
          type="button"
          aria-label={`remove ${label}`}
          className="inline-flex h-4 w-4 items-center justify-center rounded-full bg-primary/15 transition-colors hover:bg-primary/25"
          onClick={onRemove}
        >
          <X className="h-3 w-3" />
        </button>
      ) : null}
    </span>
  );
}
