"use client";

import { useLocaleText } from "@/components/i18n/locale-provider";
import { clientMarketMineHref } from "@/lib/dashboard-nav";
import type { MessageKey } from "@/lib/i18n";
import type { MarketClientContext, MarketClientHint } from "@/lib/client-market-client-status";

const hintKeys: Record<MarketClientHint, MessageKey> = {
  unknown: "dashboard.marketOfflineUnknown",
  "free-no-expiry": "dashboard.marketOfflineFreeNoExpiry",
  "long-free-no-expiry": "dashboard.marketLongOfflineFreeNoExpiry",
  "free-timed": "dashboard.marketOfflineFreeTimed",
  active: "dashboard.marketOfflineActive",
  "long-active": "dashboard.marketLongOfflineActive",
  "billing-suspended": "dashboard.marketBillingSuspended",
  released: "dashboard.marketReleased",
};

export function MarketClientBadge({ context }: { context: MarketClientContext | null }) {
  const { t } = useLocaleText();
  if (!context) return null;
  return (
    <span
      className="inline-flex shrink-0 rounded-md bg-sky-50 px-1.5 py-0.5 text-[10px] font-semibold text-sky-800"
      title={t("dashboard.marketManagedHint")}
    >
      {t("dashboard.marketManaged")}
    </span>
  );
}

export function MarketClientStatusLine({
  context,
  installationId,
}: {
  context: MarketClientContext | null;
  installationId: string;
}) {
  const { t } = useLocaleText();
  if (!context || (!context.hint && !context.manageAs)) return null;
  const needsAttention = context.hint === "long-free-no-expiry" || context.hint === "long-active";
  return (
    <span className="inline-flex min-w-0 flex-wrap items-center gap-x-2 gap-y-0.5 text-[11px]" data-no-row-drawer>
      {context.hint ? (
        <span className={needsAttention ? "font-medium text-amber-800" : "text-muted-foreground"}>
          {t(hintKeys[context.hint])}
        </span>
      ) : null}
      {context.manageAs ? (
        <a
          href={clientMarketMineHref(installationId)}
          className="shrink-0 font-medium text-accent hover:underline"
          onClick={(event) => event.stopPropagation()}
        >
          {t(context.manageAs === "client" ? "dashboard.manageMarketRental" : "dashboard.manageMarketHost")}
        </a>
      ) : null}
    </span>
  );
}
