"use client";

import * as React from "react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { formatTokenMillions, millionsInputToTokens } from "@/lib/token-units";

/** The one label style for fields across the Share settings dialog. */
export const FIELD_LABEL_CLASS = "text-xs font-medium text-muted-foreground";

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
      <span className={FIELD_LABEL_CLASS}>{label}</span>
      {children}
      {hint || invalid ? (
        <span className={`text-xs ${invalid ? "text-danger" : "text-muted-foreground"}`}>
          {invalid ? t("dashboard.fieldInvalid") : null}
          {hint && !invalid ? hint : null}
        </span>
      ) : null}
    </div>
  );
}

/**
 * Token inputs take M; Chinese pages read Tokens in 万/亿. Echo the typed value
 * in the page's unit so "4605" is never mistaken for 4,605 万.
 */
export function TokenInputEcho({ value, id }: { value: string; id?: string }) {
  const { locale, t } = useLocaleText();
  if (!/^zh\b/i.test(locale)) return null;
  const tokens = millionsInputToTokens(value);
  if (tokens == null) return null;
  return (
    <span id={id} className="text-xs tabular-nums text-muted-foreground">
      {t("dashboard.tokenInput.echo", { value: formatTokenMillions(tokens, locale) })}
    </span>
  );
}
