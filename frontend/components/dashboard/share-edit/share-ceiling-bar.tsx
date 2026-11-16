"use client";

import { Button } from "@heroui/react";
import { Pencil } from "lucide-react";
import * as React from "react";

import type { TFn } from "@/components/dashboard/share-dashboard-utils";
import { formatTokenMillions } from "@/lib/token-units";
import { ShareEditSubheading } from "./share-edit-section";

export function formatShareCeilingToken(
  value: number | undefined | null,
  unlimited: boolean,
  t: TFn,
  locale?: string,
) {
  if (unlimited) return t("common.unlimited");
  if (typeof value === "number" && Number.isFinite(value) && value > 0) {
    return formatTokenMillions(value, locale);
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

/**
 * The Share-wide ceiling: the outer bound every per-user quota sits inside.
 * It reads as one line; editing opens its three fields in place.
 */
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
  const fieldsId = React.useId();
  const summary = [
    t("dashboard.shareCeiling.token", { value: tokenDisplay }),
    t("dashboard.shareCeiling.parallel", { value: parallelDisplay }),
    t("dashboard.shareCeiling.expires", { value: expiryDisplay }),
  ].join(" · ");

  return (
    <div className="grid gap-3">
      <div className="flex min-w-0 items-start justify-between gap-3">
        <div className="grid min-w-0 gap-1">
          <ShareEditSubheading>{t("dashboard.shareCeiling.title")}</ShareEditSubheading>
          <div className="text-sm tabular-nums text-foreground [overflow-wrap:anywhere]">{summary}</div>
          <p className="text-xs leading-relaxed text-muted-foreground">{t("dashboard.shareCeiling.hint")}</p>
        </div>
        {editable ? (
          <Button
            size="sm"
            variant="ghost"
            className="shrink-0"
            aria-label={t("dashboard.shareCeiling.edit")}
            aria-expanded={open}
            aria-controls={open ? fieldsId : undefined}
            isDisabled={invalid}
            onClick={() => setExpanded((current) => !current)}
          >
            <Pencil className="h-3.5 w-3.5" aria-hidden />
            {t("common.edit")}
          </Button>
        ) : null}
      </div>
      {open ? <div id={fieldsId} className="grid gap-3 rounded-xl bg-muted/40 p-3">{children}</div> : null}
    </div>
  );
}
