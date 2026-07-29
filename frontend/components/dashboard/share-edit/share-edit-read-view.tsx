"use client";

import * as React from "react";
import { EmptyBlock, ShareUserLimitsTable } from "@/components/dashboard/drawer-panels";
import {
  expiryTitle,
  isUnlimitedParallelLimit,
  isUnlimitedTokenLimit,
  marketLabel,
  type TFn,
} from "@/components/dashboard/share-dashboard-utils";
import { getShareUserLimitStatus } from "@/lib/api";
import { shareAccessApps, resolveShareCoreApp } from "@/lib/share-app";
import type { DashboardMarket, ShareUserGrant, ShareUserLimitStatusRow, ShareView } from "@/lib/types";
import { formatDateTime } from "@/lib/utils";
import {
  forSaleOptionLabel,
  ReadOnlyChipList,
  ReadOnlyField,
  ShareEditSection,
} from "./share-edit-section";

function formatLimitDisplay(value: number | undefined | null, unlimited: boolean, t: TFn) {
  if (unlimited) return t("common.unlimited");
  if (typeof value === "number" && Number.isFinite(value) && value > 0) return String(value);
  return "—";
}

function activeUserLimitGrants(share: ShareView): ShareUserGrant[] {
  return Object.values(share.userGrants || {})
    .filter((grant) => grant.active !== false)
    .sort((left, right) => {
      if (left.role === "owner") return -1;
      if (right.role === "owner") return 1;
      return left.email.localeCompare(right.email);
    });
}

export function ShareEditReadView({
  share,
  markets,
  t,
}: {
  share: ShareView;
  markets: DashboardMarket[];
  t: TFn;
}) {
  const shareApp = resolveShareCoreApp(share) ?? shareAccessApps(share)[0];
  const tokenMarkets = markets;

  const forSale = (share.forSale as "Yes" | "No" | "Free") || "No";
  const marketAccessMode = (share.marketAccessMode as "selected" | "all") || "selected";
  const marketLinks = share.marketLinks || [];

  const selectedTokenMarketLabels = React.useMemo(() => {
    if (forSale !== "Yes" || marketAccessMode !== "selected") return [];
    return marketLinks
      .map((link) => (link.email || "").toLowerCase())
      .filter(Boolean)
      .map((email) => {
        const meta = tokenMarkets.find((market) => (market.email || "").toLowerCase() === email);
        return meta ? marketLabel(meta) : email;
      });
  }, [forSale, marketAccessMode, marketLinks, tokenMarkets]);

  const pricingPercent = shareApp ? share.forSaleOfficialPricePercentByApp?.[shareApp] : undefined;

  const tokenLimit = share.tokenLimit;
  const parallelLimit = share.parallelLimit;
  const tokenUnlimited = isUnlimitedTokenLimit(tokenLimit);
  const parallelUnlimited = isUnlimitedParallelLimit(parallelLimit);
  const limitGrants = React.useMemo(() => activeUserLimitGrants(share), [share]);
  const [limitRows, setLimitRows] = React.useState<ShareUserLimitStatusRow[] | null>(null);
  const [limitLoading, setLimitLoading] = React.useState(false);
  const [limitError, setLimitError] = React.useState("");

  React.useEffect(() => {
    if (!shareApp) {
      setLimitRows(null);
      setLimitError("");
      setLimitLoading(false);
      return;
    }
    let cancelled = false;
    setLimitLoading(true);
    setLimitError("");
    getShareUserLimitStatus(share.shareId, shareApp)
      .then((data) => {
        if (!cancelled) setLimitRows(data.rows || []);
      })
      .catch((err) => {
        if (!cancelled) {
          setLimitRows(null);
          setLimitError(err instanceof Error ? err.message : String(err));
        }
      })
      .finally(() => {
        if (!cancelled) setLimitLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [share.shareId, shareApp]);

  const marketAccessDisplay = React.useMemo(() => {
    if (forSale === "Free") return t("dashboard.publicFreeShare");
    if (forSale !== "Yes") return t("dashboard.notForSale");
    if (marketAccessMode === "all") return t("dashboard.allMarkets");
    if (selectedTokenMarketLabels.length) return null;
    return t("dashboard.noAuthorizedMarkets");
  }, [
    forSale,
    marketAccessMode,
    selectedTokenMarketLabels.length,
    t,
  ]);

  return (
    <div className="grid gap-6">
      <ShareEditSection title={t("dashboard.shareEdit.section.overview")}>
        <div className="grid gap-3 sm:grid-cols-2">
          <ReadOnlyField label={t("dashboard.field.ownerEmail")} value={share.ownerEmail || "—"} />
          <ReadOnlyField
            label={t("dashboard.field.description")}
            value={share.description?.trim() ? share.description : "—"}
          />
        </div>
      </ShareEditSection>

      {shareApp ? (
        <>
          <ShareEditSection title={t("dashboard.shareEdit.section.market")}>
            <div className="grid gap-3">
              <ReadOnlyField label={t("dashboard.field.forSale")} value={forSaleOptionLabel(forSale, t)} />
            </div>

            {forSale === "Yes" ? (
              <ReadOnlyField
                label={t("dashboard.field.marketAccess")}
                value={
                  marketAccessDisplay ?? (
                    <ReadOnlyChipList items={selectedTokenMarketLabels} />
                  )
                }
              />
            ) : null}

            {forSale === "Yes" ? (
              <ReadOnlyField
                label={t("dashboard.field.modelPricing")}
                value={
                  typeof pricingPercent === "number" && pricingPercent > 0
                    ? `${pricingPercent}%`
                    : t("common.unset")
                }
              />
            ) : null}
          </ShareEditSection>

          <ShareEditSection title={t("dashboard.shareEdit.section.access")}>
            <div className="grid gap-3 sm:grid-cols-3">
              <ReadOnlyField
                label={t("dashboard.field.tokenLimit")}
                value={formatLimitDisplay(tokenLimit, tokenUnlimited, t)}
              />
              <ReadOnlyField
                label={t("dashboard.field.parallelLimit")}
                value={formatLimitDisplay(parallelLimit, parallelUnlimited, t)}
              />
              <ReadOnlyField
                label={t("dashboard.field.expiresAt")}
                value={expiryTitle(share.expiresAt) || formatDateTime(share.expiresAt) || "—"}
              />
            </div>
            <div className="grid gap-2 border-t border-slate-200 pt-3">
              <div>
                <div className="text-sm font-semibold text-slate-900">{t("dashboard.userLimit.title")}</div>
                <p className="mt-1 text-xs text-muted-foreground">{t("dashboard.userLimit.hint")}</p>
              </div>
              {limitLoading ? (
                <EmptyBlock>{t("dashboard.userLimit.loading")}</EmptyBlock>
              ) : limitError ? (
                <EmptyBlock>{limitError}</EmptyBlock>
              ) : (limitRows?.length || limitGrants.length) ? (
                <ShareUserLimitsTable rows={limitRows || undefined} grants={limitGrants} t={t} />
              ) : (
                <EmptyBlock>{t("dashboard.userLimit.empty")}</EmptyBlock>
              )}
            </div>
          </ShareEditSection>
        </>
      ) : (
        <EmptyBlock>{t("dashboard.shareEditNoAppType")}</EmptyBlock>
      )}
    </div>
  );
}
