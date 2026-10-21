"use client";

/* Shared helpers for the Owner finance console: error text (403/409 aware),
 * code → label lookup, dates and a labelled input field. */

import * as React from "react";
import { ApiError } from "@/lib/api";
import { messages, type MessageKey } from "@/lib/i18n";
import { WALLET_INPUT } from "@/components/market/wallet-primitives";
import { cn } from "@/lib/utils";

type Translate = (key: MessageKey, values?: Record<string, string | number>) => string;

/** Localized message for an Owner action failure. */
export function ownerErrorText(t: Translate, cause: unknown) {
  if (cause instanceof ApiError && cause.status === 403) return t("settings.marketFinance.owner.ownerOnly");
  if (cause instanceof ApiError && cause.status === 409 && /version|changed|stale/i.test(cause.message)) {
    return t("settings.marketFinance.owner.conflict");
  }
  return cause instanceof Error ? cause.message : String(cause);
}

/** Translate `${prefix}${code}` when a message exists; otherwise show the raw code. */
export function codeLabel(t: Translate, prefix: string, code: string | null | undefined) {
  if (!code) return "";
  const key = `${prefix}${code}`;
  return key in messages.en ? t(key as MessageKey) : code;
}

export function ownerDateText(value: string | null | undefined, locale: string) {
  if (!value) return "";
  const date = new Date(value);
  return Number.isNaN(date.getTime())
    ? value
    : new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short" }).format(date);
}

export function OwnerField({ label, help, invalid, className, inputClassName, ...input }: {
  label: string;
  help?: React.ReactNode;
  invalid?: boolean;
  className?: string;
  inputClassName?: string;
} & Omit<React.InputHTMLAttributes<HTMLInputElement>, "className">) {
  const helpId = React.useId();
  return (
    <label className={cn("grid min-w-0 gap-1 text-sm", className)}>
      <span className="font-medium">{label}</span>
      <input
        autoComplete="off"
        spellCheck={false}
        {...input}
        aria-invalid={invalid || undefined}
        aria-describedby={help ? helpId : undefined}
        className={cn(WALLET_INPUT, inputClassName)}
      />
      {help ? <span id={helpId} className={cn("text-xs", invalid ? "text-destructive" : "text-muted-foreground")}>{help}</span> : null}
    </label>
  );
}

export const OWNER_DIALOG_CLASS = "light w-[min(560px,calc(100vw-2rem))] max-w-none !bg-white !text-slate-900";
