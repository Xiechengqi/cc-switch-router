import { MarketProviderLogos } from "@/components/dashboard/share-market/market-share-identity";
import { modelRoutingShareAccountView, modelRoutingShareCompactLine } from "@/lib/model-routing";
import type { AppLocale, MessageKey } from "@/lib/i18n";
import type { ModelRoutingApp, UserModelRoutingShare } from "@/lib/types";
import { cn } from "@/lib/utils";

export type RoutingShareSummaryProps = {
  share?: UserModelRoutingShare;
  shareId: string;
  app: ModelRoutingApp;
  locale: AppLocale;
  t: (key: MessageKey, params?: Record<string, string | number>) => string;
};

const DOT_TONES = {
  healthy: "bg-emerald-500",
  degraded: "bg-amber-500",
  unavailable: "bg-red-500",
  unknown: "bg-sky-400",
} as const;

/** Phrasing-only markup: rows sit next to buttons, never inside them. The
 * second line names provider and models; quota, account identity and the
 * health detail stay in the native tooltip instead of a third panel. */
export function RoutingShareSummary({ share, shareId, app, locale, t }: RoutingShareSummaryProps) {
  const label = share?.subdomain || share?.shareName || shareId;
  if (!share) {
    return (
      <span className="grid min-w-0 gap-0.5 text-left">
        <strong className="min-w-0 truncate font-mono text-[13px] font-medium text-muted-foreground line-through decoration-slate-300" title={label}>{label}</strong>
        <span className="truncate text-xs text-red-600">{t("modelHub.targetUnavailable")}</span>
      </span>
    );
  }
  const view = modelRoutingShareAccountView(share, app, locale, {
    passthrough: t("dashboard.modelPolicyPassthrough"), unavailable: t("dashboard.providerUnavailable"),
  });
  const compact = modelRoutingShareCompactLine(share, app, t("dashboard.modelPolicyPassthrough"));
  const status = t(share.isOnline ? "common.online" : "common.offline");
  // panelTitle keeps the observed protocol: Share health may come from another App.
  const tooltip = [label, view.primaryLine, view.identityLine, view.modelsLine,
    [view.healthLabel, view.panelTitle].filter(Boolean).join(" — ")]
    .filter(line => line && line !== "-").join("\n");
  const tone = share.isOnline ? DOT_TONES[view.healthState || "unknown"] : "bg-slate-300";
  return (
    <span className="grid min-w-0 gap-0.5 text-left" title={tooltip}>
      <span className="flex min-w-0 items-center gap-2">
        <MarketProviderLogos source={share} size={16} />
        <strong className="min-w-0 truncate font-mono text-[13px] font-medium text-foreground">{label}</strong>
        <span className="inline-flex shrink-0 items-center gap-1 text-[11px] text-muted-foreground">
          <span className={cn("model-route-dot", tone, share.isOnline && view.healthState === "healthy" && "model-route-dot-live")} aria-hidden />
          {status}
        </span>
      </span>
      <span className="truncate pl-6 text-xs text-muted-foreground">{compact || view.healthLabel}</span>
    </span>
  );
}
