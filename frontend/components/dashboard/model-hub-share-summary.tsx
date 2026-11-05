import { ShareProviderStatusPanel } from "@/components/dashboard/share-provider-status-panel";
import { MarketProviderLogos } from "@/components/dashboard/share-market/market-share-identity";
import { ShareAppLogo } from "@/components/dashboard/share-app-logo";
import { modelRoutingShareAccountView } from "@/lib/model-routing";
import type { AppLocale, MessageKey } from "@/lib/i18n";
import type { ModelRoutingApp, UserModelRoutingShare } from "@/lib/types";

export type RoutingShareSummaryProps = {
  share?: UserModelRoutingShare;
  shareId: string;
  app: ModelRoutingApp;
  locale: AppLocale;
  t: (key: MessageKey, params?: Record<string, string | number>) => string;
};

function accountView({ share, app, locale, t }: RoutingShareSummaryProps) {
  return share ? modelRoutingShareAccountView(share, app, locale, {
    passthrough: t("dashboard.modelPolicyPassthrough"), unavailable: t("dashboard.providerUnavailable"),
  }) : {
    primaryLine: "—", identityLine: t("modelHub.targetUnavailable"), modelsLine: "—",
    toneClassName: "border-slate-200 bg-slate-50 text-slate-600",
    healthLabel: t("modelHub.targetUnavailable"),
  };
}

/** Phrasing-only markup is safe inside both buttons and CompactSelect options. */
export function RoutingShareSummary(props: RoutingShareSummaryProps) {
  const { share, shareId, app, t } = props;
  const label = share?.subdomain || share?.shareName || shareId;
  return (
    <span className="model-route-account grid gap-2 text-left">
      <span className="flex min-w-0 items-center gap-1.5">
        {share ? <MarketProviderLogos source={share} size={16} /> : null}
        <strong className="min-w-0 flex-1 truncate font-mono text-xs text-foreground" title={label}>{label}</strong>
        <ShareAppLogo app={app} size={14} />
        {share ? <span className="shrink-0 text-[11px] text-muted-foreground">{t(share.isOnline ? "common.online" : "common.offline")}</span> : null}
      </span>
      <ShareProviderStatusPanel as="span" view={accountView(props)} className="model-route-health" />
    </span>
  );
}

/** Open menu item. The closed trigger shows the Share name only. */
export function RoutingShareOption(props: RoutingShareSummaryProps) {
  const { share, shareId, app } = props;
  const label = share?.subdomain || share?.shareName || shareId;
  const view = accountView(props);
  return (
    <span className="grid min-w-0 gap-0.5 text-left">
      <span className="flex min-w-0 items-center gap-1.5">
        {share ? <MarketProviderLogos source={share} size={14} /> : null}
        <strong className="min-w-0 flex-1 truncate font-mono text-xs text-foreground" title={label}>{label}</strong>
        <ShareAppLogo app={app} size={12} />
      </span>
      <span className="truncate text-[11px] text-slate-500" title={view.primaryLine}>{view.primaryLine}</span>
      <span className="truncate text-[11px] text-slate-500" title={view.identityLine}>{view.identityLine}</span>
      <span className="truncate text-[11px] text-slate-500" title={view.modelsLine}>{view.modelsLine}</span>
    </span>
  );
}

/** Details stay outside the add/remove/drag controls: touch can inspect long
 * account text without accidentally adding a Share to the queue. */
export function RoutingShareDetails(props: RoutingShareSummaryProps) {
  if (!props.share) return null;
  const view = accountView(props);
  return (
    <details className="model-route-account text-xs text-muted-foreground">
      <summary className="flex min-h-11 cursor-pointer items-center rounded px-2 underline underline-offset-4 hover:bg-secondary active:bg-secondary focus-visible:outline-2 focus-visible:outline-ring">
        {props.t("modelHub.accountDetails")}
      </summary>
      <div className="grid gap-1 break-words px-2 pb-2 [overflow-wrap:anywhere]">
        <span>{props.share.subdomain || props.share.shareName}</span>
        <span>{view.primaryLine}</span><span>{view.identityLine}</span><span>{view.modelsLine}</span>
        <span>{view.panelTitle || view.healthLabel}</span>
      </div>
    </details>
  );
}
