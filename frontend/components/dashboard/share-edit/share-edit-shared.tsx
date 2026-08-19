"use client";

import * as React from "react";
import { useLocaleText } from "@/components/i18n/locale-provider";

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
