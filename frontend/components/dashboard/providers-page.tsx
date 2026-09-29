"use client";

/* Hallmark · genre: modern-minimal · macrostructure: Workbench · theme: existing router system · enrichment: none · nav: existing app shell · footer: none
 * audience: Router market users · task: compare Provider trust, then open a market or top up · tone: technical
 * pre-emit critique: P5 H5 E4 S5 R5 V4
 */

import * as React from "react";
import Link from "next/link";
import { usePathname, useRouter, useSearchParams } from "next/navigation";
import { Button, Drawer } from "@heroui/react";
import {
  AlertTriangle,
  ArrowUpRight,
  BadgeCheck,
  BarChart3,
  ChevronRight,
  CircleDollarSign,
  Gauge,
  Loader2,
  RefreshCw,
  Search,
  WalletCards,
} from "lucide-react";
import { useAuth } from "@/components/auth/auth-provider";
import { SegmentedControl } from "@/components/common/segmented-control";
import { MarketFundingTopupDialog } from "@/components/dashboard/market-funding-topup-dialog";
import { useLocaleText } from "@/components/i18n/locale-provider";
import {
  getMarketProvider,
  getMarketProviders,
  getMyMarketProviderFunding,
} from "@/lib/api";
import { clientMarketHref, shareMarketHref } from "@/lib/dashboard-nav";
import { marketFundingTopupUnavailableKey } from "@/lib/market-funding";
import { formatUsdMoney } from "@/lib/market-money";
import type {
  BinanceFundingIntent,
  MarketProvider,
  MarketProviderDetail,
  MarketProviderList,
  MyMarketProviderFunding,
  MyMarketProviderFundingResponse,
} from "@/lib/types";
import {
  actorBoundValue,
  filterMarketProviders,
  formatBasisPointScore,
  isFormallyRanked,
  preferredProviderTopupFunding,
  providerCreditSummary,
  type ProviderMarketFilter,
} from "@/components/dashboard/providers-page-utils";

function formatDate(value: string | undefined, locale: string) {
  if (!value) return "—";
  const date = new Date(value);
  if (Number.isNaN(date.valueOf())) return "—";
  return new Intl.DateTimeFormat(locale, { dateStyle: "medium" }).format(date);
}

function formatDateTime(value: string | undefined, locale: string) {
  if (!value) return "—";
  const date = new Date(value);
  if (Number.isNaN(date.valueOf())) return "—";
  return new Intl.DateTimeFormat(locale, {
    dateStyle: "medium",
    timeStyle: "short",
  }).format(date);
}

function ProviderBadges({
  provider,
  showNewProvider = false,
}: {
  provider: Pick<MarketProvider, "official" | "exploration">;
  showNewProvider?: boolean;
}) {
  const { t } = useLocaleText();
  if (!provider.official && !(showNewProvider && provider.exploration)) return null;
  return (
    <span className="flex min-w-0 flex-wrap items-center gap-1.5">
      {provider.official ? (
        <span className="inline-flex h-6 items-center gap-1 whitespace-nowrap rounded-full bg-blue-50 px-2 text-[11px] font-semibold text-blue-700">
          <BadgeCheck className="h-3.5 w-3.5" aria-hidden="true" />
          {t("providers.official")}
        </span>
      ) : null}
      {showNewProvider && provider.exploration ? (
        <span className="inline-flex h-6 items-center whitespace-nowrap rounded-full bg-amber-50 px-2 text-[11px] font-medium text-amber-800">
          {t("providers.exploration")}
        </span>
      ) : null}
    </span>
  );
}

function RankMark({ provider }: { provider: MarketProvider }) {
  const { t } = useLocaleText();
  if (!isFormallyRanked(provider)) {
    return (
      <span className="inline-flex min-h-7 items-center whitespace-nowrap rounded-full bg-slate-100 px-2.5 text-xs font-medium text-slate-600">
        {t("providers.collecting")}
      </span>
    );
  }
  return (
    <span className="inline-flex min-w-11 items-baseline gap-0.5 font-mono text-lg font-semibold text-slate-950 tabular-nums">
      <span className="text-xs font-medium text-slate-500">#</span>
      {provider.rankPosition}
    </span>
  );
}

function ScoreComponents({ provider }: { provider: MarketProvider }) {
  const { locale, t } = useLocaleText();
  const values = [
    [t("providers.component.serviceQuality"), provider.components.serviceQualityBps],
    [t("providers.component.effectiveChoice"), provider.components.effectiveChoiceBps],
    [t("providers.component.fulfillment"), provider.components.fulfillmentBps],
    [t("providers.component.supplyBreadth"), provider.components.supplyBreadthBps],
  ] as const;
  return (
    <dl className="grid min-w-0 grid-cols-2 gap-x-4 gap-y-2 sm:grid-cols-4">
      {values.map(([label, value]) => (
        <div key={label} className="min-w-0">
          <dt className="truncate text-[11px] leading-4 text-slate-500" title={label}>{label}</dt>
          <dd className="mt-0.5 font-mono text-xs font-semibold tabular-nums text-slate-800">
            {formatBasisPointScore(value, locale)}
          </dd>
        </div>
      ))}
    </dl>
  );
}

function ProviderRankRow({
  provider,
  onOpen,
}: {
  provider: MarketProvider;
  onOpen: (providerId: string) => void;
}) {
  const { locale, t } = useLocaleText();
  const ranked = isFormallyRanked(provider);
  const supply = [
    provider.inventory.hasShareMarket
      ? {
        key: "share",
        available: provider.inventory.availableShareSeats,
        label: t("providers.supply.shareAvailable", {
          count: provider.inventory.availableShareSeats,
        }),
      }
      : null,
    provider.inventory.hasClientMarket
      ? {
        key: "client",
        available: provider.inventory.idleHostTotal,
        label: t("providers.supply.clientAvailable", {
          count: provider.inventory.idleHostTotal,
        }),
      }
      : null,
  ].filter((item): item is { key: string; available: number; label: string } => !!item);
  return (
    <button
      type="button"
      className="grid min-h-11 w-full min-w-0 grid-cols-[minmax(0,1fr)_auto] gap-x-4 gap-y-3 px-4 py-4 text-left outline-none transition-colors hover:bg-slate-50 active:bg-slate-100 focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-primary sm:grid-cols-[auto_minmax(0,1fr)_auto] sm:items-center sm:px-5"
      onClick={() => onOpen(provider.id)}
    >
      <span className="col-start-1 row-start-1 self-start sm:self-center">
        <RankMark provider={provider} />
      </span>
      <span className="col-span-2 col-start-1 row-start-2 grid min-w-0 gap-2 sm:col-span-1 sm:col-start-2 sm:row-start-1">
        <span className="flex min-w-0 flex-wrap items-center gap-2">
          <strong className="min-w-0 [overflow-wrap:anywhere] text-sm text-slate-950">
            {provider.displayName}
          </strong>
          <ProviderBadges provider={provider} />
        </span>
        {ranked ? (
          <span className="flex min-w-0 flex-wrap gap-x-3 gap-y-1 text-xs text-slate-600">
            <span>
              {t("providers.component.serviceQuality")}{" "}
              <strong className="font-mono font-semibold tabular-nums text-slate-800">
                {formatBasisPointScore(provider.components.serviceQualityBps, locale)}
              </strong>
            </span>
            <span>
              {t("providers.fulfillmentValue", {
                successes: provider.quality.fulfillmentSuccesses,
                total: provider.quality.fulfillmentTotal,
              })}
            </span>
          </span>
        ) : (
          <span className="flex min-w-0 flex-wrap gap-x-3 gap-y-1 text-xs text-slate-500">
            <span>{t("providers.independentBuyersValue", { count: provider.independentBuyerCount })}</span>
            <span>{t("providers.observationDaysValue", { count: provider.observationDays })}</span>
          </span>
        )}
        <span className="flex min-w-0 flex-wrap gap-x-3 gap-y-1 text-xs text-slate-600">
          {supply.length ? supply.map((item) => (
            <span key={item.key} className="inline-flex items-center gap-1.5 whitespace-nowrap">
              <span
                className={item.available > 0 ? "h-1.5 w-1.5 rounded-full bg-emerald-500" : "h-1.5 w-1.5 rounded-full bg-slate-300"}
                aria-hidden="true"
              />
              {item.label}
            </span>
          )) : (
            <span>{t("providers.supply.noneAvailable")}</span>
          )}
        </span>
      </span>
      <span className="col-start-2 row-start-1 flex shrink-0 items-center justify-end gap-3 sm:col-start-3">
        {ranked ? (
          <span className="text-right">
            <span className="block text-[11px] text-slate-500">{t("providers.score")}</span>
            <strong className="mt-0.5 block font-mono text-base font-semibold tabular-nums text-slate-950 sm:text-lg">
              {formatBasisPointScore(provider.scoreBps, locale)}
            </strong>
          </span>
        ) : null}
        <span className="hidden items-center gap-1 whitespace-nowrap text-xs font-semibold text-primary md:inline-flex">
          {t("providers.viewSupply")}
          <ChevronRight className="h-4 w-4" aria-hidden="true" />
        </span>
        <ChevronRight className="h-4 w-4 text-slate-400 md:hidden" aria-hidden="true" />
      </span>
    </button>
  );
}

function LoadingRows() {
  return (
    <div className="grid" aria-hidden="true">
      {[0, 1, 2].map((index) => (
        <div key={index} className="grid gap-3 border-t border-slate-100 px-4 py-5 first:border-t-0">
          <div className="h-4 w-40 animate-pulse rounded bg-slate-100" />
          <div className="h-3 w-full max-w-xl animate-pulse rounded bg-slate-100" />
        </div>
      ))}
    </div>
  );
}

function FundingTotals({ funding }: { funding: MyMarketProviderFundingResponse }) {
  const { locale, t } = useLocaleText();
  const totals = funding.totals;
  const cells = [
    [t("providers.funding.providerCount"), String(totals.providerCount)],
    [t("providers.funding.prepaidAvailable"), formatUsdMoney(totals.prepaidAvailableMinor, locale)],
    [t("providers.funding.prepaidHeld"), formatUsdMoney(totals.prepaidHeldMinor, locale)],
    [t("providers.funding.finiteCredit"), formatUsdMoney(totals.finiteCreditAvailableMinor, locale)],
    [t("providers.funding.creditOutstanding"), formatUsdMoney(totals.creditOutstandingMinor, locale)],
    [t("providers.funding.unlimitedCredit"), String(totals.unlimitedCreditProviderCount)],
    [t("providers.funding.legacyDailyRate"), formatUsdMoney(totals.legacyDailyRateMinor, locale)],
    [t("providers.funding.monthlyCommitment"), formatUsdMoney(totals.monthlyCommitmentMinor, locale)],
    [t("providers.funding.nextRenewal"), totals.nextRenewalAt
      ? `${formatDate(totals.nextRenewalAt, locale)} · ${formatUsdMoney(totals.nextRenewalMinor, locale)}`
      : "—"],
  ] as const;
  return (
    <dl className="grid grid-cols-2 gap-x-5 gap-y-4 bg-slate-50 px-4 py-4 sm:grid-cols-3 lg:grid-cols-5">
      {cells.map(([label, value]) => (
        <div key={label} className="min-w-0">
          <dt className="text-[11px] leading-4 text-slate-500">{label}</dt>
          <dd className="mt-1 truncate font-mono text-sm font-semibold tabular-nums text-slate-950" title={value}>
            {value}
          </dd>
        </div>
      ))}
    </dl>
  );
}

function creditValue(
  credit: ReturnType<typeof providerCreditSummary>,
  locale: string,
  unlimited: string,
) {
  if (credit.kind === "unlimited") return unlimited;
  if (credit.kind === "none") return "—";
  return formatUsdMoney(credit.availableMinor, locale);
}

function FundingRow({
  provider,
  onTopup,
}: {
  provider: MyMarketProviderFunding;
  onTopup: (provider: MyMarketProviderFunding) => void;
}) {
  const { locale, t } = useLocaleText();
  const topupFunding = preferredProviderTopupFunding(provider);
  const credit = providerCreditSummary(provider);
  return (
    <article className="grid min-w-0 gap-4 border-t border-slate-100 px-4 py-4 first:border-t-0 lg:grid-cols-[minmax(11rem,0.7fr)_minmax(0,1.6fr)_auto] lg:items-center">
      <div className="min-w-0">
        <div className="flex min-w-0 flex-wrap items-center gap-2">
          <h3 className="min-w-0 break-words text-sm font-semibold text-slate-950">
            {provider.displayName}
          </h3>
          {provider.official ? (
            <span className="inline-flex h-6 items-center gap-1 whitespace-nowrap rounded-full bg-blue-50 px-2 text-[11px] font-semibold text-blue-700">
              <BadgeCheck className="h-3.5 w-3.5" aria-hidden="true" />
              {t("providers.official")}
            </span>
          ) : null}
        </div>
        <p className="mt-1 truncate text-xs text-slate-500" title={provider.supplierEmail}>
          {provider.supplierEmail}
        </p>
      </div>
      <dl className="grid min-w-0 grid-cols-2 gap-x-4 gap-y-3 sm:grid-cols-3 xl:grid-cols-6">
        <div className="min-w-0">
          <dt className="text-[11px] text-slate-500">{t("providers.funding.prepaidAvailable")}</dt>
          <dd className="mt-0.5 truncate font-mono text-xs font-semibold tabular-nums">
            {formatUsdMoney(provider.shareFunding.prepaidAvailableMinor, locale)}
          </dd>
        </div>
        <div className="min-w-0">
          <dt className="text-[11px] text-slate-500">{t("providers.funding.prepaidHeld")}</dt>
          <dd className="mt-0.5 truncate font-mono text-xs font-semibold tabular-nums">
            {formatUsdMoney(provider.shareFunding.prepaidHeldMinor, locale)}
          </dd>
        </div>
        <div className="min-w-0">
          <dt className="text-[11px] text-slate-500">{t("providers.funding.creditAvailable")}</dt>
          <dd className="mt-0.5 truncate font-mono text-xs font-semibold tabular-nums">
            {creditValue(credit, locale, t("common.unlimited"))}
          </dd>
        </div>
        <div className="min-w-0">
          <dt className="text-[11px] text-slate-500">{t("providers.funding.shareRate")}</dt>
          <dd className="mt-0.5 truncate font-mono text-xs font-semibold tabular-nums">
            {formatUsdMoney(provider.shareLegacyDailyRateMinor, locale)}
          </dd>
        </div>
        <div className="min-w-0">
          <dt className="text-[11px] text-slate-500">{t("providers.funding.clientRate")}</dt>
          <dd className="mt-0.5 truncate font-mono text-xs font-semibold tabular-nums">
            {formatUsdMoney(provider.clientLegacyDailyRateMinor, locale)}
          </dd>
        </div>
        <div className="min-w-0">
          <dt className="text-[11px] text-slate-500">{t("providers.funding.monthlyCommitment")}</dt>
          <dd className="mt-0.5 truncate font-mono text-xs font-semibold tabular-nums">
            {formatUsdMoney(provider.recurring.monthlyCommitmentMinor, locale)}
          </dd>
        </div>
      </dl>
      <div className="grid min-w-0 justify-items-start gap-1 lg:justify-items-end">
        <Button
          size="sm"
          variant="outline"
          className="min-h-11 shrink-0 whitespace-nowrap"
          isDisabled={!topupFunding.topupAvailable}
          aria-describedby={topupFunding.topupAvailable ? undefined : `provider-topup-${provider.marketProviderId}`}
          onClick={() => onTopup(provider)}
        >
          <CircleDollarSign className="h-4 w-4" aria-hidden="true" />
          {t("providers.funding.topup")}
        </Button>
        {!topupFunding.topupAvailable ? (
          <p
            id={`provider-topup-${provider.marketProviderId}`}
            className="max-w-56 text-xs leading-5 text-slate-500 lg:text-right"
          >
            {t(marketFundingTopupUnavailableKey(topupFunding.topupUnavailableReason))}
          </p>
        ) : null}
      </div>
    </article>
  );
}

function ProviderDetailDrawer({
  providerId,
  summary,
  detail,
  loading,
  error,
  onClose,
}: {
  providerId: string;
  summary?: MarketProvider;
  detail: MarketProviderDetail | null;
  loading: boolean;
  error: string;
  onClose: () => void;
}) {
  const { locale, t } = useLocaleText();
  const provider = detail ?? summary;
  return (
    <Drawer.Backdrop isOpen={!!providerId} onOpenChange={(open) => !open && onClose()}>
      <Drawer.Content placement="right">
        <Drawer.Dialog className="light h-full w-[min(680px,calc(100vw-0.5rem))] max-w-none !bg-white !text-slate-900">
          <Drawer.CloseTrigger className="!h-11 !w-11 !bg-slate-100 !text-slate-700 hover:!bg-slate-200 active:!bg-slate-300" />
          <Drawer.Header>
            <div className="min-w-0 pr-10">
              <Drawer.Heading className="min-w-0 [overflow-wrap:anywhere] text-lg">
                {provider?.displayName || t("providers.detail.title")}
              </Drawer.Heading>
              {provider ? <div className="mt-2"><ProviderBadges provider={provider} showNewProvider /></div> : null}
            </div>
          </Drawer.Header>
          <Drawer.Body className="grid content-start gap-6 overflow-y-auto pb-8">
            {loading && !provider ? (
              <div className="flex min-h-40 items-center justify-center gap-2 text-sm text-slate-500">
                <Loader2 className="h-4 w-4 animate-spin" aria-hidden="true" />
                {t("common.loading")}
              </div>
            ) : null}
            {error ? (
              <p role="alert" className="rounded-lg bg-rose-50 px-3 py-2 text-sm text-rose-800">
                {error}
              </p>
            ) : null}
            {provider ? (
              <>
                <section className="grid gap-4">
                  <div className="flex min-w-0 flex-wrap items-end justify-between gap-3">
                    <div>
                      <span className="block text-xs text-slate-500">{t("providers.score")}</span>
                      <strong className="mt-1 block font-mono text-3xl font-semibold tabular-nums">
                        {isFormallyRanked(provider)
                          ? formatBasisPointScore(provider.scoreBps, locale)
                          : "—"}
                      </strong>
                    </div>
                    <RankMark provider={provider} />
                  </div>
                  <ScoreComponents provider={provider} />
                  <dl className="grid grid-cols-2 gap-3 text-sm sm:grid-cols-4">
                    <div><dt className="text-xs text-slate-500">{t("providers.independentBuyers")}</dt><dd className="mt-1 font-mono font-semibold tabular-nums">{provider.independentBuyerCount}</dd></div>
                    <div><dt className="text-xs text-slate-500">{t("providers.observationDays")}</dt><dd className="mt-1 font-mono font-semibold tabular-nums">{provider.observationDays}</dd></div>
                    <div><dt className="text-xs text-slate-500">{t("providers.quality")}</dt><dd className="mt-1 font-mono font-semibold tabular-nums">{provider.quality.fulfillmentSuccesses}/{provider.quality.fulfillmentTotal}</dd></div>
                    <div><dt className="text-xs text-slate-500">{t("providers.joinedAt")}</dt><dd className="mt-1 font-medium">{formatDate(provider.joinedAt, locale)}</dd></div>
                  </dl>
                </section>

                <section className="grid gap-3 rounded-lg bg-slate-50 px-3 py-3">
                  <div className="flex items-center gap-2">
                    <Gauge className="h-4 w-4 text-slate-500" aria-hidden="true" />
                    <h3 className="text-sm font-semibold">{t("providers.performance.title")}</h3>
                  </div>
                  <dl className="grid grid-cols-2 gap-3">
                    <div><dt className="text-xs text-slate-500">TTFT</dt><dd className="mt-1 font-mono text-sm font-semibold tabular-nums">{provider.performance.averageTtftMs == null ? "—" : `${Math.round(provider.performance.averageTtftMs)} ms`}</dd></div>
                    <div><dt className="text-xs text-slate-500">TPS</dt><dd className="mt-1 font-mono text-sm font-semibold tabular-nums">{provider.performance.averageTps == null ? "—" : provider.performance.averageTps.toFixed(1)}</dd></div>
                  </dl>
                  <p className="text-xs leading-5 text-slate-600">{t("providers.performance.displayOnly")}</p>
                </section>

                {detail ? (
                  <section className="grid gap-5">
                    <div className="grid gap-2">
                      <div className="flex min-w-0 flex-wrap items-center justify-between gap-3">
                        <h3 className="text-sm font-semibold">{t("providers.detail.shareSupply")}</h3>
                        {provider.inventory.hasShareMarket ? (
                          <Link
                            href={shareMarketHref({ providerId: provider.id })}
                            className="inline-flex min-h-11 shrink-0 items-center gap-1 whitespace-nowrap text-sm font-medium text-primary outline-none hover:underline active:text-primary/70 focus-visible:ring-2 focus-visible:ring-primary"
                          >
                            {t("providers.viewShareMarket")}
                            <ArrowUpRight className="h-4 w-4" aria-hidden="true" />
                          </Link>
                        ) : null}
                      </div>
                      {detail.shareSupply.length ? (
                        <div className="grid gap-1">
                          {detail.shareSupply.map((supply) => (
                            <div key={supply.listingId} className="grid min-w-0 gap-1 rounded-md bg-slate-50 px-3 py-2 text-sm sm:grid-cols-[minmax(0,1fr)_auto] sm:gap-3">
                              <span className="min-w-0"><strong className="block truncate">{supply.shareName}</strong><span className="block truncate text-xs text-slate-500">{supply.apps.join(" · ") || "—"}</span></span>
                              <span className="whitespace-nowrap font-mono text-xs tabular-nums text-slate-600">{t("providers.detail.seatsValue", { available: supply.availableSeats, total: supply.totalSeats })}</span>
                            </div>
                          ))}
                        </div>
                      ) : <p className="text-sm text-slate-500">{t("providers.detail.noShareSupply")}</p>}
                    </div>

                    <div className="grid gap-2">
                      <div className="flex min-w-0 flex-wrap items-center justify-between gap-3">
                        <h3 className="text-sm font-semibold">{t("providers.detail.clientSupply")}</h3>
                        {provider.inventory.hasClientMarket ? (
                          <Link
                            href={clientMarketHref({ providerId: provider.id })}
                            className="inline-flex min-h-11 shrink-0 items-center gap-1 whitespace-nowrap text-sm font-medium text-primary outline-none hover:underline active:text-primary/70 focus-visible:ring-2 focus-visible:ring-primary"
                          >
                            {t("providers.viewClientMarket")}
                            <ArrowUpRight className="h-4 w-4" aria-hidden="true" />
                          </Link>
                        ) : null}
                      </div>
                      {detail.clientSupply.length ? (
                        <div className="grid gap-1 sm:grid-cols-2">
                          {detail.clientSupply.map((supply) => (
                            <div key={supply.countryCode} className="grid min-w-0 gap-1 rounded-md bg-slate-50 px-3 py-2 text-sm sm:grid-cols-[minmax(0,1fr)_auto] sm:items-center sm:gap-3">
                              <strong>{supply.countryCode}</strong>
                              <span className="whitespace-nowrap font-mono text-xs tabular-nums text-slate-600">{t("providers.detail.hostsValue", { idle: supply.idleHosts, total: supply.totalHosts })}</span>
                            </div>
                          ))}
                        </div>
                      ) : <p className="text-sm text-slate-500">{t("providers.detail.noClientSupply")}</p>}
                    </div>
                  </section>
                ) : null}
              </>
            ) : null}
          </Drawer.Body>
        </Drawer.Dialog>
      </Drawer.Content>
    </Drawer.Backdrop>
  );
}

function RankingMethodDrawer({
  open,
  generation,
  onClose,
}: {
  open: boolean;
  generation: MarketProviderList["generation"] | null;
  onClose: () => void;
}) {
  const { locale, t } = useLocaleText();
  const components = generation ? [
    [t("providers.component.serviceQuality"), generation.weights.serviceQuality],
    [t("providers.component.effectiveChoice"), generation.weights.effectiveChoice],
    [t("providers.component.fulfillment"), generation.weights.fulfillment],
    [t("providers.component.supplyBreadth"), generation.weights.supplyBreadth],
  ] as const : [];
  return (
    <Drawer.Backdrop isOpen={open} onOpenChange={(nextOpen) => !nextOpen && onClose()}>
      <Drawer.Content placement="right">
        <Drawer.Dialog className="light h-full w-[min(520px,calc(100vw-0.5rem))] max-w-none !bg-white !text-slate-900">
          <Drawer.CloseTrigger className="!h-11 !w-11 !bg-slate-100 !text-slate-700 hover:!bg-slate-200 active:!bg-slate-300" />
          <Drawer.Header>
            <Drawer.Heading className="pr-10 text-lg">
              {t("providers.method.title")}
            </Drawer.Heading>
          </Drawer.Header>
          <Drawer.Body className="grid content-start gap-6 overflow-y-auto pb-8">
            {!generation ? (
              <div className="flex min-h-40 items-center justify-center gap-2 text-sm text-slate-500">
                <Loader2 className="h-4 w-4 animate-spin" aria-hidden="true" />
                {t("common.loading")}
              </div>
            ) : (
              <>
                <p className="text-sm leading-6 text-slate-600">
                  {t("providers.method.summary")}
                </p>

                <section className="grid gap-3" aria-labelledby="provider-method-weights">
                  <h3 id="provider-method-weights" className="text-sm font-semibold text-slate-950">
                    {t("providers.method.weights")}
                  </h3>
                  <dl className="grid grid-cols-2 gap-2">
                    {components.map(([label, weight]) => (
                      <div key={label} className="rounded-lg bg-slate-50 px-3 py-3">
                        <dt className="text-xs text-slate-500">{label}</dt>
                        <dd className="mt-1 font-mono text-lg font-semibold tabular-nums text-slate-950">
                          {t("providers.method.weightValue", { weight })}
                        </dd>
                      </div>
                    ))}
                  </dl>
                </section>

                <section className="grid gap-2" aria-labelledby="provider-method-eligibility">
                  <h3 id="provider-method-eligibility" className="text-sm font-semibold text-slate-950">
                    {t("providers.method.eligibility")}
                  </h3>
                  <p className="text-sm leading-6 text-slate-600">
                    {t("providers.method.eligibilityText", {
                      buyers: generation.minimumIndependentBuyers,
                      days: generation.minimumObservationDays,
                    })}
                  </p>
                </section>

                <section className="grid gap-2 rounded-lg bg-slate-50 px-3 py-3" aria-labelledby="provider-method-notes">
                  <h3 id="provider-method-notes" className="text-sm font-semibold text-slate-950">
                    {t("providers.method.notes")}
                  </h3>
                  <ul className="grid list-disc gap-2 pl-5 text-sm leading-6 text-slate-600">
                    <li>{t("providers.method.officialNote")}</li>
                    <li>{t("providers.method.filterNote")}</li>
                    <li>{t("providers.method.performanceNote")}</li>
                  </ul>
                </section>

                <dl className="grid gap-2 text-xs text-slate-500">
                  <div className="flex min-w-0 items-baseline justify-between gap-4">
                    <dt>{t("providers.method.published")}</dt>
                    <dd className="text-right text-slate-700">
                      {formatDateTime(generation.publishedAt, locale)}
                    </dd>
                  </div>
                  <div className="flex min-w-0 items-baseline justify-between gap-4">
                    <dt>{t("providers.method.version")}</dt>
                    <dd className="min-w-0 break-all text-right font-mono text-slate-700">
                      {generation.algorithmVersion}
                    </dd>
                  </div>
                </dl>
              </>
            )}
          </Drawer.Body>
        </Drawer.Dialog>
      </Drawer.Content>
    </Drawer.Backdrop>
  );
}

function ProviderFundingDrawer({
  open,
  authLoading,
  authed,
  funding,
  loading,
  error,
  onClose,
  onLogin,
  onReload,
  onTopup,
}: {
  open: boolean;
  authLoading: boolean;
  authed: boolean;
  funding: MyMarketProviderFundingResponse | null;
  loading: boolean;
  error: string;
  onClose: () => void;
  onLogin: () => void;
  onReload: () => void;
  onTopup: (provider: MyMarketProviderFunding) => void;
}) {
  const { t } = useLocaleText();
  return (
    <Drawer.Backdrop isOpen={open} onOpenChange={(nextOpen) => !nextOpen && onClose()}>
      <Drawer.Content placement="right">
        <Drawer.Dialog className="light h-full w-[min(900px,calc(100vw-0.5rem))] max-w-none !bg-white !text-slate-900">
          <Drawer.CloseTrigger className="!h-11 !w-11 !bg-slate-100 !text-slate-700 hover:!bg-slate-200 active:!bg-slate-300" />
          <Drawer.Header>
            <div className="min-w-0 pr-10">
              <Drawer.Heading className="flex items-center gap-2 text-lg">
                <WalletCards className="h-4 w-4 text-slate-500" aria-hidden="true" />
                {t("providers.funding.title")}
              </Drawer.Heading>
              <p className="mt-1 max-w-2xl text-xs leading-5 text-slate-500">
                {t("providers.funding.subtitle")}
              </p>
            </div>
          </Drawer.Header>
          <Drawer.Body className="grid content-start gap-4 overflow-y-auto pb-8">
            {authLoading ? (
              <div className="flex min-h-40 items-center justify-center gap-2 text-sm text-slate-500">
                <Loader2 className="h-4 w-4 animate-spin" aria-hidden="true" />
                {t("common.loading")}
              </div>
            ) : null}

            {!authLoading && !authed ? (
              <div className="grid min-h-40 place-items-center gap-4 rounded-xl bg-slate-50 px-5 py-8 text-center">
                <div className="max-w-md">
                  <strong className="block text-sm text-slate-950">
                    {t("providers.funding.loginTitle")}
                  </strong>
                  <span className="mt-1 block text-sm leading-6 text-slate-600">
                    {t("providers.funding.loginPrompt")}
                  </span>
                </div>
                <Button variant="primary" className="min-h-11 whitespace-nowrap" onClick={onLogin}>
                  {t("providers.funding.loginAction")}
                </Button>
              </div>
            ) : null}

            {authed ? (
              <div className="min-w-0 overflow-hidden rounded-xl border border-slate-200 bg-white" aria-busy={loading}>
                {funding ? <FundingTotals funding={funding} /> : null}
                {loading && !funding ? <LoadingRows /> : null}
                {error ? (
                  <div className="grid min-h-32 place-items-center gap-3 px-4 py-8 text-center">
                    <p role="alert" className="max-w-xl text-sm text-rose-700">{error}</p>
                    <Button variant="outline" size="sm" className="min-h-11 whitespace-nowrap" onClick={onReload}>
                      {t("common.retry")}
                    </Button>
                  </div>
                ) : null}
                {!loading && !error && funding?.providers.length === 0 ? (
                  <div className="grid min-h-32 place-items-center px-4 py-8 text-center text-sm text-slate-500">
                    {t("providers.funding.empty")}
                  </div>
                ) : null}
                {funding?.providers.map((provider) => (
                  <FundingRow key={provider.marketProviderId} provider={provider} onTopup={onTopup} />
                ))}
              </div>
            ) : null}

            {authed && funding ? (
              <Button
                variant="ghost"
                size="sm"
                className="min-h-11 justify-self-start whitespace-nowrap"
                isDisabled={loading}
                onClick={onReload}
              >
                {loading ? <Loader2 className="h-4 w-4 animate-spin" aria-hidden="true" /> : <RefreshCw className="h-4 w-4" aria-hidden="true" />}
                {t("common.reload")}
              </Button>
            ) : null}
          </Drawer.Body>
        </Drawer.Dialog>
      </Drawer.Content>
    </Drawer.Backdrop>
  );
}

export function ProvidersPage() {
  const { locale, t } = useLocaleText();
  const { session, loading: authLoading } = useAuth();
  const pathname = usePathname();
  const router = useRouter();
  const searchParams = useSearchParams();
  const authed = !!session?.authenticated;
  const actorKey = session?.user?.id || session?.user?.email || "anonymous";
  const providerId = searchParams.get("provider") || "";

  const [providerList, setProviderList] = React.useState<MarketProviderList | null>(null);
  const [providerLoading, setProviderLoading] = React.useState(true);
  const [providerError, setProviderError] = React.useState("");
  const [detailSnapshot, setDetailSnapshot] = React.useState<{
    actorKey: string;
    value: MarketProviderDetail;
  } | null>(null);
  const [detailLoadingFor, setDetailLoadingFor] = React.useState<string | null>(null);
  const [detailErrorSnapshot, setDetailErrorSnapshot] = React.useState<{
    actorKey: string;
    value: string;
  } | null>(null);
  const [fundingSnapshot, setFundingSnapshot] = React.useState<{
    actorKey: string;
    value: MyMarketProviderFundingResponse;
  } | null>(null);
  const [fundingLoading, setFundingLoading] = React.useState(false);
  const [fundingErrorSnapshot, setFundingErrorSnapshot] = React.useState<{
    actorKey: string;
    value: string;
  } | null>(null);
  const [fundingOpen, setFundingOpen] = React.useState(false);
  const [methodOpen, setMethodOpen] = React.useState(false);
  const [topupProvider, setTopupProvider] = React.useState<MyMarketProviderFunding | null>(null);
  const [query, setQuery] = React.useState("");
  const [marketFilter, setMarketFilter] = React.useState<ProviderMarketFilter>("all");
  const listAbortRef = React.useRef<AbortController | null>(null);
  const fundingAbortRef = React.useRef<AbortController | null>(null);
  // Session effects run after render. Bind private data to the actor in state
  // as well, so an account switch cannot paint the previous actor's balances
  // or leave their top-up dialog visible for even one frame.
  const funding = actorBoundValue(fundingSnapshot, actorKey);
  const fundingError = actorBoundValue(fundingErrorSnapshot, actorKey) || "";
  const detail = actorBoundValue(detailSnapshot, providerId);
  const detailError = actorBoundValue(detailErrorSnapshot, providerId) || "";
  const detailLoading = detailLoadingFor === providerId;
  const visibleTopupProvider = funding && topupProvider
    ? funding.providers.find(
      (provider) => provider.marketProviderId === topupProvider.marketProviderId,
    ) ?? null
    : null;

  const loadProviders = React.useCallback(async () => {
    listAbortRef.current?.abort();
    const controller = new AbortController();
    listAbortRef.current = controller;
    setProviderLoading(true);
    setProviderError("");
    try {
      const next = await getMarketProviders(controller.signal);
      if (!controller.signal.aborted) setProviderList(next);
    } catch (reason) {
      if (!controller.signal.aborted) {
        setProviderError(reason instanceof Error ? reason.message : String(reason));
      }
    } finally {
      if (listAbortRef.current === controller) {
        listAbortRef.current = null;
        setProviderLoading(false);
      }
    }
  }, []);

  const loadFunding = React.useCallback(async () => {
    if (!authed) return;
    fundingAbortRef.current?.abort();
    const controller = new AbortController();
    fundingAbortRef.current = controller;
    setFundingLoading(true);
    setFundingErrorSnapshot(null);
    try {
      const next = await getMyMarketProviderFunding(controller.signal);
      if (!controller.signal.aborted) {
        setFundingSnapshot({ actorKey, value: next });
      }
    } catch (reason) {
      if (!controller.signal.aborted) {
        setFundingErrorSnapshot({
          actorKey,
          value: reason instanceof Error ? reason.message : String(reason),
        });
      }
    } finally {
      if (fundingAbortRef.current === controller) {
        fundingAbortRef.current = null;
        setFundingLoading(false);
      }
    }
  }, [actorKey, authed]);

  React.useEffect(() => {
    void loadProviders();
    return () => listAbortRef.current?.abort();
  }, [loadProviders]);

  React.useEffect(() => {
    if (authLoading) return;
    // Never leave one authenticated actor's balances visible while another
    // actor's private request is in flight. Funding loads lazily when opened.
    fundingAbortRef.current?.abort();
    fundingAbortRef.current = null;
    setFundingSnapshot(null);
    setFundingErrorSnapshot(null);
    setFundingLoading(false);
    setTopupProvider(null);
    return () => fundingAbortRef.current?.abort();
  }, [actorKey, authed, authLoading]);

  React.useEffect(() => {
    if (!fundingOpen || authLoading || !authed || funding || fundingLoading || fundingError) {
      return;
    }
    void loadFunding();
  }, [authed, authLoading, funding, fundingError, fundingLoading, fundingOpen, loadFunding]);

  React.useEffect(() => {
    if (!providerId) {
      setDetailSnapshot(null);
      setDetailErrorSnapshot(null);
      setDetailLoadingFor(null);
      return;
    }
    const controller = new AbortController();
    setDetailSnapshot(null);
    setDetailErrorSnapshot(null);
    setDetailLoadingFor(providerId);
    void getMarketProvider(providerId, controller.signal)
      .then((next) => {
        if (!controller.signal.aborted) {
          setDetailSnapshot({ actorKey: providerId, value: next });
        }
      })
      .catch((reason) => {
        if (!controller.signal.aborted) {
          setDetailErrorSnapshot({
            actorKey: providerId,
            value: reason instanceof Error ? reason.message : String(reason),
          });
        }
      })
      .finally(() => {
        if (!controller.signal.aborted) setDetailLoadingFor(null);
      });
    return () => controller.abort();
  }, [providerId]);

  const updateProviderQuery = React.useCallback((nextProviderId?: string) => {
    const params = new URLSearchParams(searchParams.toString());
    if (nextProviderId) params.set("provider", nextProviderId);
    else params.delete("provider");
    const search = params.toString();
    router.replace(`${pathname}${search ? `?${search}` : ""}`, { scroll: false });
  }, [pathname, router, searchParams]);

  const visibleProviders = React.useMemo(
    () => filterMarketProviders(providerList?.providers ?? [], {
      query,
      market: marketFilter,
    }),
    [marketFilter, providerList?.providers, query],
  );
  const selectedSummary = providerList?.providers.find((provider) => provider.id === providerId);
  const filtersActive = !!query || marketFilter !== "all";

  const resetFilters = () => {
    setQuery("");
    setMarketFilter("all");
  };

  const openLogin = () => window.dispatchEvent(new Event("router-open-login"));
  const handleCredited = async (_intent: BinanceFundingIntent) => {
    setTopupProvider(null);
    await loadFunding();
    setFundingOpen(true);
  };
  const handleTopup = (provider: MyMarketProviderFunding) => {
    setTopupProvider(provider);
    setFundingOpen(false);
  };
  const closeTopup = () => {
    setTopupProvider(null);
    setFundingOpen(true);
  };

  return (
    <main className="mx-auto grid w-[calc(100%-2rem)] min-w-0 max-w-7xl gap-8 pb-12">
      <header className="grid min-w-0 gap-4 sm:grid-cols-[minmax(0,1fr)_auto] sm:items-end">
        <div className="min-w-0">
          <h1 className="min-w-0 [overflow-wrap:anywhere] text-2xl font-bold tracking-tight text-slate-950">
            {t("providers.title")}
          </h1>
          <p className="mt-1 max-w-3xl text-sm leading-6 text-slate-600">
            {t("providers.subtitle")}
          </p>
        </div>
        <div className="flex min-w-0 flex-wrap items-center gap-2 sm:justify-end">
          <Button
            variant="outline"
            size="sm"
            className="min-h-11 whitespace-nowrap"
            onClick={() => setFundingOpen(true)}
          >
            <WalletCards className="h-4 w-4" aria-hidden="true" />
            {t("providers.funding.title")}
          </Button>
          <Button
            variant="ghost"
            size="sm"
            className="min-h-11 whitespace-nowrap"
            isDisabled={providerLoading}
            onClick={() => void loadProviders()}
          >
            {providerLoading ? <Loader2 className="h-4 w-4 animate-spin" aria-hidden="true" /> : <RefreshCw className="h-4 w-4" aria-hidden="true" />}
            {t("common.reload")}
          </Button>
        </div>
      </header>

      {providerList?.generation.stale ? (
        <div className="flex min-w-0 items-start gap-2 rounded-lg bg-amber-50 px-3 py-2 text-sm text-amber-900" role="status">
          <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0" aria-hidden="true" />
          <span>{t("providers.generation.stale", { time: formatDateTime(providerList.generation.publishedAt, locale) })}</span>
        </div>
      ) : null}

      <section className="grid min-w-0 gap-4" aria-labelledby="provider-ranking-title">
        <div className="flex min-w-0 flex-wrap items-center justify-between gap-3">
          <h2 id="provider-ranking-title" className="text-base font-semibold text-slate-950">
            {t("providers.rankingTitle")}
          </h2>
          <div className="flex min-w-0 flex-wrap items-center justify-end gap-2">
            <span className="text-xs tabular-nums text-slate-500">
              {t("providers.results", { count: visibleProviders.length })}
            </span>
            <Button
              variant="ghost"
              size="sm"
              className="min-h-11 whitespace-nowrap"
              isDisabled={!providerList}
              onClick={() => setMethodOpen(true)}
            >
              <BarChart3 className="h-4 w-4" aria-hidden="true" />
              {t("providers.method.open")}
            </Button>
          </div>
        </div>

        <div className="flex min-w-0 flex-col gap-3 sm:flex-row sm:items-center">
          <label className="relative min-w-0 flex-1 sm:max-w-md">
            <span className="sr-only">{t("providers.search.label")}</span>
            <Search className="pointer-events-none absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-slate-500" aria-hidden="true" />
            <input
              type="search"
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              placeholder={t("providers.search.placeholder")}
              disabled={providerLoading && !providerList}
              className="min-h-11 w-full rounded-lg border border-slate-200 bg-white py-2 pl-9 pr-3 text-sm text-slate-950 outline-2 outline-offset-1 outline-transparent placeholder:text-slate-400 hover:bg-slate-50 active:bg-slate-100 focus-visible:outline-primary disabled:cursor-not-allowed disabled:bg-slate-50 disabled:text-slate-400 disabled:opacity-[0.55]"
            />
          </label>
          <div className="flex min-w-0 flex-wrap items-center gap-2">
            <SegmentedControl
              value={marketFilter}
              onChange={setMarketFilter}
              ariaLabel={t("providers.market.label")}
              size="md"
              disabled={providerLoading && !providerList}
              items={[
                { id: "all", label: t("providers.market.all") },
                { id: "share", label: t("providers.market.share") },
                { id: "client", label: t("providers.market.client") },
              ]}
            />
            {filtersActive ? (
              <Button
                variant="ghost"
                size="sm"
                className="min-h-11 whitespace-nowrap"
                onClick={resetFilters}
              >
                {t("providers.filters.clear")}
              </Button>
            ) : null}
          </div>
        </div>

        <div className="min-w-0 overflow-hidden rounded-xl border border-slate-200 bg-white" aria-busy={providerLoading}>
          {providerLoading && !providerList ? <LoadingRows /> : null}
          {providerError ? (
            <div className="grid min-h-40 place-items-center gap-3 px-4 py-8 text-center">
              <p role="alert" className="max-w-xl text-sm text-rose-700">{providerError}</p>
              <Button variant="outline" size="sm" className="min-h-11 whitespace-nowrap" onClick={() => void loadProviders()}>
                {t("common.retry")}
              </Button>
            </div>
          ) : null}
          {!providerLoading && !providerError && visibleProviders.length === 0 ? (
            <div className="grid min-h-40 place-items-center gap-3 px-4 py-8 text-center text-sm text-slate-500">
              <p>{filtersActive ? t("providers.empty.filtered") : t("providers.empty.all")}</p>
              {filtersActive ? (
                <Button variant="ghost" size="sm" className="min-h-11 whitespace-nowrap" onClick={resetFilters}>
                  {t("providers.filters.clear")}
                </Button>
              ) : null}
            </div>
          ) : null}
          {visibleProviders.map((provider, index) => (
            <div key={provider.id} className={index ? "border-t border-slate-100" : undefined}>
              <ProviderRankRow provider={provider} onOpen={updateProviderQuery} />
            </div>
          ))}
        </div>
      </section>

      <ProviderDetailDrawer
        providerId={providerId}
        summary={selectedSummary}
        detail={detail}
        loading={detailLoading}
        error={detailError}
        onClose={() => updateProviderQuery()}
      />

      <RankingMethodDrawer
        open={methodOpen}
        generation={providerList?.generation ?? null}
        onClose={() => setMethodOpen(false)}
      />

      <ProviderFundingDrawer
        open={fundingOpen}
        authLoading={authLoading}
        authed={authed}
        funding={funding}
        loading={fundingLoading}
        error={fundingError}
        onClose={() => setFundingOpen(false)}
        onLogin={openLogin}
        onReload={() => void loadFunding()}
        onTopup={handleTopup}
      />

      <MarketFundingTopupDialog
        open={!!visibleTopupProvider}
        funding={visibleTopupProvider ? preferredProviderTopupFunding(visibleTopupProvider) : undefined}
        onClose={closeTopup}
        onCredited={handleCredited}
      />
    </main>
  );
}
