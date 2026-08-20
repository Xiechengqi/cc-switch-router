"use client";

import { Button } from "@heroui/react";
import { Pencil } from "lucide-react";
import * as React from "react";

import type { TFn } from "@/components/dashboard/share-dashboard-utils";
import { compactTokens } from "@/lib/utils";

export function formatShareCeilingToken(
  value: number | undefined | null,
  unlimited: boolean,
  t: TFn,
) {
  if (unlimited) return t("common.unlimited");
  if (typeof value === "number" && Number.isFinite(value) && value > 0) {
    return compactTokens(value);
  }
  return "—";
}

export function formatShareCeilingParallel(
  value: number | undefined | null,
  unlimited: boolean,
  t: TFn,
) {
  if (unlimited) return t("common.unlimited");
  if (typeof value === "number" && Number.isFinite(value) && value > 0) {
    return String(value);
  }
  return "—";
}

export function ShareCeilingBar({
  t,
  tokenDisplay,
  parallelDisplay,
  expiryDisplay,
  editable,
  invalid,
  children,
}: {
  t: TFn;
  tokenDisplay: string;
  parallelDisplay: string;
  expiryDisplay: string;
  editable?: boolean;
  invalid?: boolean;
  children?: React.ReactNode;
}) {
  const [expanded, setExpanded] = React.useState(false);
  const open = Boolean(editable && (expanded || invalid));
  const summary = [
    t("dashboard.shareCeiling.token", { value: tokenDisplay }),
    t("dashboard.shareCeiling.parallel", { value: parallelDisplay }),
    t("dashboard.shareCeiling.expires", { value: expiryDisplay }),
  ].join(" · ");

  return (
    <div className="grid gap-2 rounded-lg border border-slate-200 bg-slate-50 px-3 py-2">
      <div className="flex min-w-0 items-start justify-between gap-2">
        <div className="min-w-0 grid gap-0.5">
          <div className="text-[11px] font-semibold uppercase tracking-[0.08em] text-slate-500">
            {t("dashboard.shareCeiling.title")}
          </div>
          <div className="text-sm font-medium text-slate-900">{summary}</div>
          <p className="text-xs text-muted-foreground">{t("dashboard.shareCeiling.hint")}</p>
        </div>
        {editable ? (
          <Button
            isIconOnly
            size="sm"
            variant="ghost"
            className="h-6 w-6 min-w-6 shrink-0"
            aria-label={t("dashboard.shareCeiling.edit")}
            aria-expanded={open}
            onClick={() => setExpanded((current) => !current)}
          >
            <Pencil className="h-3.5 w-3.5" />
          </Button>
        ) : null}
      </div>
      {open ? <div className="grid gap-3 border-t border-slate-200 pt-2">{children}</div> : null}
    </div>
  );
}
