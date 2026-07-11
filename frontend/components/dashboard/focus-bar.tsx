"use client";

import { X } from "lucide-react";
import { useDashboardFocus } from "@/components/dashboard/dashboard-focus";
import { useLocaleText } from "@/components/i18n/locale-provider";

export function FocusBar() {
  const { target, label, relatedClientIds, relatedShareIds, relatedMarketIds, clearFocus } = useDashboardFocus();
  const { t } = useLocaleText();
  if (!target) return null;
  return (
    <div className="flex items-center justify-between gap-4 rounded-lg border border-primary/20 bg-primary/[0.04] px-3 py-2 text-xs" role="status">
      <div className="min-w-0 truncate">
        <span className="text-muted-foreground">{t("dashboard.viewing")}: </span>
        <strong className="text-foreground">{t(`dashboard.focus.${target.kind}` as const)} “{label}”</strong>
        <span className="ml-2 text-muted-foreground">· {relatedClientIds.size} Clients · {relatedShareIds.size} Shares · {relatedMarketIds.size} Markets</span>
      </div>
      <button type="button" onClick={clearFocus} className="inline-flex h-7 shrink-0 items-center gap-1 rounded-md px-2 text-muted-foreground hover:bg-white hover:text-foreground" aria-label={t("dashboard.clearFocus")}>
        <X className="h-3.5 w-3.5" />{t("dashboard.clearFocus")}
      </button>
    </div>
  );
}
