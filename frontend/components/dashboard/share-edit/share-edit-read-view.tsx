"use client";

import * as React from "react";
import { EmptyBlock } from "@/components/dashboard/drawer-panels";
import {
  expiryTitle,
  isShareMarket,
  isUnlimitedParallelLimit,
  isUnlimitedTokenLimit,
  marketLabel,
  type TFn,
} from "@/components/dashboard/share-dashboard-utils";
import { shareAccessApps, resolveShareCoreApp } from "@/lib/share-app";
import type { DashboardMarket, ShareView } from "@/lib/types";
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
  const tokenMarkets = React.useMemo(() => markets.filter((market) => !isShareMarket(market)), [markets]);
  const shareMarkets = React.useMemo(() => markets.filter(isShareMarket), [markets]);
  const tokenMarketEmails = React.useMemo(
    () => new Set(tokenMarkets.map((market) => (market.email || "").toLowerCase()).filter(Boolean)),
    [tokenMarkets],
  );
  const shareMarketEmails = React.useMemo(
    () => new Set(shareMarkets.map((market) => (market.email || "").toLowerCase()).filter(Boolean)),
    [shareMarkets],
  );

  const forSale = (share.forSale as "Yes" | "No" | "Free") || "No";
  const saleMarketKind = share.saleMarketKind === "share" ? "share" : "token";
  const marketAccessMode = (share.marketAccessMode as "selected" | "all") || "selected";
  const marketLinks = share.marketLinks || [];

  const selectedTokenMarketLabels = React.useMemo(() => {
    if (forSale !== "Yes" || saleMarketKind !== "token" || marketAccessMode !== "selected") return [];
    return marketLinks
      .map((link) => (link.email || "").toLowerCase())
      .filter((email) => email && !shareMarketEmails.has(email))
      .map((email) => {
        const meta = tokenMarkets.find((market) => (market.email || "").toLowerCase() === email);
        return meta ? marketLabel(meta) : email;
      });
  }, [forSale, marketAccessMode, marketLinks, saleMarketKind, shareMarketEmails, tokenMarkets]);

  const selectedShareMarketLabel = React.useMemo(() => {
    if (forSale !== "Yes" || saleMarketKind !== "share") return "";
    const email = marketLinks
      .map((link) => (link.email || "").toLowerCase())
      .find((value) => value && !tokenMarketEmails.has(value));
    if (!email) return "";
    const meta = shareMarkets.find((market) => (market.email || "").toLowerCase() === email);
    return meta ? marketLabel(meta) : email;
  }, [forSale, marketLinks, saleMarketKind, shareMarkets, tokenMarketEmails]);

  const pricingPercent = shareApp ? share.forSaleOfficialPricePercentByApp?.[shareApp] : undefined;

  const tokenLimit = share.tokenLimit;
  const parallelLimit = share.parallelLimit;
  const tokenUnlimited = isUnlimitedTokenLimit(tokenLimit);
  const parallelUnlimited = isUnlimitedParallelLimit(parallelLimit);
  const currentGrant = Object.values(share.userGrants || {}).find(
    (grant) => grant.active !== false,
  );
  const userPeriodLabel = currentGrant
    ? {
        lifetime: t("dashboard.userLimit.periodLifetime"),
        day: t("dashboard.userLimit.periodDay"),
        week: t("dashboard.userLimit.periodWeek"),
        calendarMonth: t("dashboard.userLimit.periodMonth"),
      }[currentGrant.policy.tokenPeriod]
    : "—";

  const marketAccessDisplay = React.useMemo(() => {
    if (forSale === "Free") return t("dashboard.publicFreeShare");
    if (forSale !== "Yes") return t("dashboard.notForSale");
    if (saleMarketKind === "share") {
      return selectedShareMarketLabel || t("dashboard.selectShareMarket");
    }
    if (marketAccessMode === "all") return t("dashboard.allMarkets");
    if (selectedTokenMarketLabels.length) return null;
    return t("dashboard.noAuthorizedMarkets");
  }, [
    forSale,
    marketAccessMode,
    saleMarketKind,
    selectedShareMarketLabel,
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
            <div className="grid gap-3 sm:grid-cols-2">
              <ReadOnlyField label={t("dashboard.field.forSale")} value={forSaleOptionLabel(forSale, t)} />
              {forSale === "Yes" ? (
                <ReadOnlyField
                  label={t("dashboard.field.marketType")}
                  value={saleMarketKind === "share" ? t("dashboard.shareMarket") : t("dashboard.tokenMarket")}
                />
              ) : null}
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

            {forSale === "Yes" && saleMarketKind === "token" ? (
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
            {currentGrant ? (
              <div className="grid gap-3 border-t border-slate-200 pt-3 sm:grid-cols-2 lg:grid-cols-4">
                <ReadOnlyField label="Email" value={currentGrant.email} />
                <ReadOnlyField
                  label={t("dashboard.field.parallelLimit")}
                  value={formatLimitDisplay(
                    currentGrant.policy.parallelLimit,
                    currentGrant.policy.parallelLimit == null,
                    t,
                  )}
                />
                <ReadOnlyField
                  label={t("dashboard.field.tokenLimit")}
                  value={`${formatLimitDisplay(
                    currentGrant.policy.tokenLimit,
                    currentGrant.policy.tokenLimit == null,
                    t,
                  )} · ${userPeriodLabel}`}
                />
                <ReadOnlyField
                  label={t("dashboard.field.expiresAt")}
                  value={
                    currentGrant.policy.expiresAt
                      ? formatDateTime(new Date(currentGrant.policy.expiresAt).toISOString())
                      : t("dashboard.permanent")
                  }
                />
              </div>
            ) : null}
          </ShareEditSection>
        </>
      ) : (
        <EmptyBlock>{t("dashboard.shareEditNoAppType")}</EmptyBlock>
      )}
    </div>
  );
}
