"use client";

import { Card, Chip } from "@heroui/react";
import { ExternalLink } from "lucide-react";
import * as React from "react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import {
  averageRecentLatencyMs,
  formatLatencySeconds,
  type CoreShareApp,
  ShareAppSupportCard,
  ShareConnectChip,
  ShareEditAction,
  ShareExceptionalStatusBadge,
  ShareMarketListingStatusChip,
  shareApiParts,
  shareAppSettings,
  shareAppTokensUsed,
  shareExpiryProgress,
  shareStatusShareMarketUrl,
  UsageBar,
} from "@/components/dashboard/data-tables";
import type { ShareRequestLog, ShareView } from "@/lib/types";
import { compactTokens } from "@/lib/utils";

const SHARE_APP_LABELS: Record<CoreShareApp, string> = {
  claude: "Claude",
  codex: "Codex",
  gemini: "Gemini",
};

function coreShareApp(value?: string): CoreShareApp | null {
  const normalized = String(value || "").trim().toLowerCase();
  return normalized === "claude" || normalized === "codex" || normalized === "gemini"
    ? normalized
    : null;
}

function requestBelongsToApp(request: ShareRequestLog, app: CoreShareApp) {
  const appType = (request.appType || "").trim().toLowerCase();
  if (appType) return appType === app;
  return (request.requestAgent || "").trim().toLowerCase() === app;
}

function isUnlimited(value?: number) {
  return Number(value) < 0;
}

export const ShareCard = React.memo(function ShareCard({
  share,
  onOpen,
  onEdit,
  onConnect,
}: {
  share: ShareView;
  onOpen: (share: ShareView) => void;
  onEdit: (share: ShareView) => void;
  onConnect: (share: ShareView) => void;
}) {
  const { locale, t } = useLocaleText();
  const app = coreShareApp(share.appType);
  const api = shareApiParts(share);
  const settings = app ? shareAppSettings(share, app) : null;
  const appRequests = app ? (share.recentRequests || []).filter((request) => requestBelongsToApp(request, app)) : share.recentRequests || [];
  const tokensUsed = app ? shareAppTokensUsed(share, app) : share.tokensUsed || 0;
  const tokenLimit = settings?.tokenLimit ?? share.tokenLimit;
  const parallelLimit = settings?.parallelLimit ?? share.parallelLimit;
  const activeRequests = app ? share.activeRequestsByApp?.[app] ?? 0 : share.activeRequests || 0;
  const forSale = settings?.forSale ?? share.forSale;
  const saleMarketKind = settings?.saleMarketKind ?? share.saleMarketKind;
  const averageLatency = averageRecentLatencyMs(appRequests);
  const saleValue =
    forSale === "Free"
      ? t("dashboard.free")
      : forSale === "Yes"
        ? saleMarketKind === "share"
          ? t("dashboard.shareMarket")
          : t("dashboard.tokenMarket")
        : t("dashboard.no");
  const saleVariant: "soft" | "tertiary" = forSale === "No" ? "tertiary" : "soft";
  const shareMarketListingUrl = app ? shareStatusShareMarketUrl(share, app) : null;
  const effectiveShare = settings ? { ...share, expiresAt: settings.expiresAt } : share;
  const rowClass = "grid grid-cols-[58px_minmax(0,1fr)] gap-2 text-[11px]";

  return (
    <Card
      className="w-72 shrink-0 snap-start cursor-pointer select-none rounded-lg border border-default/50 bg-white p-0 shadow-sm transition-colors hover:border-primary/30 hover:bg-primary/[0.03]"
      onClick={() => onOpen(share)}
    >
      <Card.Content className="grid gap-3 p-3">
        <div className="grid min-w-0 gap-1.5">
          <div className="flex min-w-0 items-start justify-between gap-2">
            <strong className="min-w-0 break-all font-mono text-xs text-foreground">
              {api.apiUrl}
            </strong>
            <ShareExceptionalStatusBadge share={share} t={t} />
          </div>
          <div className="flex min-w-0 flex-wrap items-center gap-1.5">
            <ShareConnectChip share={share} onOpen={onConnect} t={t} />
            <ShareEditAction share={share} onEdit={onEdit} t={t} />
          </div>
        </div>

        {app ? (
          <ShareAppSupportCard share={share} app={app} label={SHARE_APP_LABELS[app]} locale={locale} />
        ) : (
          <div className="rounded-lg border bg-slate-50 px-2 py-1.5 text-[11px] text-muted-foreground">
            {share.appType || t("dashboard.appType")}
          </div>
        )}

        <div className="grid min-w-0 gap-2 text-sm">
          <div className={rowClass}>
            <span className="mono-label text-muted-foreground">{t("dashboard.forSale")}</span>
            <div className="flex min-w-0 flex-wrap items-center gap-1">
              {shareMarketListingUrl ? (
                <a
                  href={shareMarketListingUrl}
                  target="_blank"
                  rel="noreferrer"
                  data-no-row-drawer
                  className="inline-flex items-center gap-1"
                  title={shareMarketListingUrl}
                  onClick={(event) => event.stopPropagation()}
                >
                  <Chip size="sm" variant={saleVariant}>
                    {saleValue}
                    <ExternalLink className="ml-1 inline h-3 w-3" />
                  </Chip>
                </a>
              ) : (
                <Chip size="sm" variant={saleVariant}>{saleValue}</Chip>
              )}
              {app ? <ShareMarketListingStatusChip share={share} app={app} t={t} /> : null}
            </div>
          </div>
          <div className={rowClass}>
            <span className="mono-label text-muted-foreground">{t("dashboard.usage")}</span>
            <div>
              <strong>{compactTokens(tokensUsed)} / {isUnlimited(tokenLimit) ? "∞" : compactTokens(tokenLimit)}</strong>
              <UsageBar used={tokensUsed} limit={tokenLimit} t={t} />
            </div>
          </div>
          <div className={rowClass}>
            <span className="mono-label text-muted-foreground">{t("dashboard.expires")}</span>
            <strong title={settings?.expiresAt || share.expiresAt}>
              {shareExpiryProgress(effectiveShare, locale)}
            </strong>
          </div>
          <div className={rowClass}>
            <span className="mono-label text-muted-foreground">{t("dashboard.parallel")}</span>
            <strong>{activeRequests}<span className="text-muted-foreground">/{isUnlimited(parallelLimit) ? "∞" : parallelLimit || 0}</span></strong>
          </div>
          <div className={rowClass}>
            <span className="mono-label text-muted-foreground">{t("dashboard.response")}</span>
            <strong>{formatLatencySeconds(averageLatency)}</strong>
          </div>
        </div>
      </Card.Content>
    </Card>
  );
});
