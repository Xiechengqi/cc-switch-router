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
  CircleDollarSign,
  Gauge,
  Loader2,
  RefreshCw,
  Search,
  WalletCards,
} from "lucide-react";
import { useAuth } from "@/components/auth/auth-provider";
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
  basisPointsPercent,
  filterMarketProviders,
  preferredProviderTopupFunding,
  providerCreditSummary,
  type ProviderMarketFilter,
  type ProviderRankFilter,
} from "@/components/dashboard/providers-page-utils";

function formatPercent(value: number | undefined, locale: string) {
  const percent = basisPointsPercent(value);
  if (percent == null) return "—";
  return new Intl.NumberFormat(locale, {
    maximumFractionDigits: 1,
    minimumFractionDigits: percent > 0 && percent < 10 ? 1 : 0,
  }).format(percent) + "%";
}

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

function ProviderBadges({ provider }: { provider: Pick<MarketProvider, "official" | "exploration"> }) {
  const { t } = useLocaleText();
  return (
    <span className="flex min-w-0 flex-wrap items-center gap-1.5">
      {provider.official ? (
        <span className="inline-flex h-6 items-center gap-1 whitespace-nowrap rounded-full bg-blue-50 px-2 text-[11px] font-semibold text-blue-700">
          <BadgeCheck className="h-3.5 w-3.5" aria-hidden="true" />
          {t("providers.official")}
        </span>
      ) : null}
      {provider.exploration ? (
        <span className="inline-flex h-6 items-center whitespace-nowrap rounded-full bg-amber-50 px-2 text-[11px] font-medium text-amber-800">
          {t("providers.exploration")}
        </span>
      ) : null}
    </span>
  );
}

function RankMark({ provider }: { provider: MarketProvider }) {
  const { t } = useLocaleText();
  if (provider.rankState !== "ranked" || provider.rankPosition == null) {
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
            {formatPercent(value, locale)}
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
  return (
    <button
      type="button"
      className="grid min-h-11 w-full min-w-0 gap-4 px-4 py-4 text-left outline-none transition-colors hover:bg-slate-50 active:bg-slate-100 focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-primary lg:grid-cols-[minmax(12rem,1fr)_minmax(8rem,0.45fr)_minmax(20rem,1.35fr)] lg:items-center"
      onClick={() => onOpen(provider.id)}
    >
      <span className="grid min-w-0 grid-cols-[auto_minmax(0,1fr)] items-start gap-3">
        <RankMark provider={provider} />
        <span className="min-w-0">
          <span className="flex min-w-0 flex-wrap items-center gap-2">
            <strong className="min-w-0 [overflow-wrap:anywhere] text-sm text-slate-950">
              {provider.displayName}
            </strong>
            <ProviderBadges provider={provider} />
          </span>
          <span className="mt-1 flex min-w-0 flex-wrap gap-x-3 gap-y-1 text-xs text-slate-500">
            <span>{t("providers.independentBuyersValue", { count: provider.independentBuyerCount })}</span>
            <span>{t("providers.observationDaysValue", { count: provider.observationDays })}</span>
          </span>
        </span>
      </span>
      <span className="min-w-0">
        <span className="block text-[11px] text-slate-500">{t("providers.score")}</span>
        <strong className="mt-0.5 block font-mono text-xl font-semibold tabular-nums text-slate-950">
          {formatPercent(provider.scoreBps, locale)}
        </strong>
        <span className="mt-1 block text-xs text-slate-500">
          {provider.inventory.hasShareMarket
            ? t("providers.supply.shareValue", {
              listings: provider.inventory.activeShareCount,
              seats: provider.inventory.availableShareSeats,
            })
            : t("providers.supply.noShare")}
          {" · "}
          {provider.inventory.hasClientMarket
            ? t("providers.supply.clientValue", {
              hosts: provider.inventory.hostTotal,
              idle: provider.inventory.idleHostTotal,
            })
            : t("providers.supply.noClient")}
        </span>
      </span>
      <ScoreComponents provider={provider} />
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
              {provider ? <div className="mt-2"><ProviderBadges provider={provider} /></div> : null}
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
                        {formatPercent(provider.scoreBps, locale)}
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
  const [topupProvider, setTopupProvider] = React.useState<MyMarketProviderFunding | null>(null);
  const [query, setQuery] = React.useState("");
  const [marketFilter, setMarketFilter] = React.useState<ProviderMarketFilter>("all");
  const [rankFilter, setRankFilter] = React.useState<ProviderRankFilter>("all");
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
    if (!authed) {
      fundingAbortRef.current?.abort();
      fundingAbortRef.current = null;
      setFundingSnapshot(null);
      setFundingErrorSnapshot(null);
      setFundingLoading(false);
      setTopupProvider(null);
      return;
    }
    // Never leave one authenticated actor's balances visible while another
    // actor's private request is in flight.
    setFundingSnapshot(null);
    setTopupProvider(null);
    void loadFunding();
    return () => fundingAbortRef.current?.abort();
  }, [actorKey, authed, authLoading, loadFunding]);

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
      rank: rankFilter,
    }),
    [marketFilter, providerList?.providers, query, rankFilter],
  );
  const selectedSummary = providerList?.providers.find((provider) => provider.id === providerId);
  const filtersActive = !!query || marketFilter !== "all" || rankFilter !== "all";

  const resetFilters = () => {
    setQuery("");
    setMarketFilter("all");
    setRankFilter("all");
  };

  const openLogin = () => window.dispatchEvent(new Event("router-open-login"));
  const handleCredited = async (_intent: BinanceFundingIntent) => {
    setTopupProvider(null);
    await loadFunding();
  };

  return (
    <main className="mx-auto grid w-[calc(100%-2rem)] min-w-0 max-w-7xl gap-8 pb-12">
      <header className="grid min-w-0 gap-2 sm:grid-cols-[minmax(0,1fr)_auto] sm:items-end">
        <div className="min-w-0">
          <h1 className="min-w-0 [overflow-wrap:anywhere] text-2xl font-bold tracking-tight text-slate-950">
            {t("providers.title")}
          </h1>
          <p className="mt-1 max-w-3xl text-sm leading-6 text-slate-600">
            {t("providers.subtitle")}
          </p>
        </div>
        <Button
          variant="outline"
          size="sm"
          className="min-h-11 justify-self-start whitespace-nowrap sm:justify-self-end"
          isDisabled={providerLoading}
          onClick={() => void loadProviders()}
        >
          {providerLoading ? <Loader2 className="h-4 w-4 animate-spin" aria-hidden="true" /> : <RefreshCw className="h-4 w-4" aria-hidden="true" />}
          {t("common.reload")}
        </Button>
      </header>

      {providerList?.generation.stale ? (
        <div className="flex min-w-0 items-start gap-2 rounded-lg bg-amber-50 px-3 py-2 text-sm text-amber-900" role="status">
          <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0" aria-hidden="true" />
          <span>{t("providers.generation.stale", { time: formatDateTime(providerList.generation.publishedAt, locale) })}</span>
        </div>
      ) : providerList ? (
        <div className="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1 text-xs text-slate-500">
          <span className="inline-flex items-center gap-1.5 text-emerald-700">
            <BarChart3 className="h-3.5 w-3.5" aria-hidden="true" />
            {t("providers.generation.current")}
          </span>
          <span>{t("providers.generation.published", { time: formatDateTime(providerList.generation.publishedAt, locale) })}</span>
          <span className="font-mono">{providerList.generation.algorithmVersion}</span>
        </div>
      ) : null}

      <section className="grid min-w-0 gap-4" aria-labelledby="provider-ranking-title">
        <div className="flex min-w-0 flex-wrap items-end justify-between gap-3">
          <div>
            <h2 id="provider-ranking-title" className="text-base font-semibold text-slate-950">
              {t("providers.rankingTitle")}
            </h2>
            <p className="mt-1 text-xs leading-5 text-slate-500">{t("providers.rankingHint")}</p>
          </div>
          <span className="text-xs tabular-nums text-slate-500">
            {t("providers.results", { count: visibleProviders.length })}
          </span>
        </div>

        <div className="grid min-w-0 gap-3 sm:grid-cols-2 lg:grid-cols-[minmax(14rem,1fr)_12rem_12rem_auto]">
          <label className="grid min-w-0 gap-1 text-xs font-medium text-slate-600">
            <span>{t("providers.search.label")}</span>
            <span className="relative min-w-0">
              <Search className="pointer-events-none absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-slate-500" aria-hidden="true" />
              <input
                type="search"
                value={query}
                onChange={(event) => setQuery(event.target.value)}
                placeholder={t("providers.search.placeholder")}
                disabled={providerLoading && !providerList}
                className="min-h-11 w-full rounded-lg border border-slate-200 bg-white py-2 pl-9 pr-3 text-sm text-slate-950 outline-2 outline-offset-1 outline-transparent placeholder:text-slate-400 hover:bg-slate-50 active:bg-slate-100 focus-visible:outline-primary disabled:cursor-not-allowed disabled:bg-slate-50 disabled:text-slate-400 disabled:opacity-[0.55]"
              />
            </span>
          </label>
          <label className="grid min-w-0 gap-1 text-xs font-medium text-slate-600">
            <span>{t("providers.market.label")}</span>
            <select
              value={marketFilter}
              onChange={(event) => setMarketFilter(event.target.value as ProviderMarketFilter)}
              disabled={providerLoading && !providerList}
              className="min-h-11 w-full rounded-lg border border-slate-200 bg-white px-3 text-sm text-slate-950 outline-2 outline-offset-1 outline-transparent hover:bg-slate-50 active:bg-slate-100 focus-visible:outline-primary disabled:cursor-not-allowed disabled:bg-slate-50 disabled:text-slate-400 disabled:opacity-[0.55]"
            >
              <option value="all">{t("providers.market.all")}</option>
              <option value="share">{t("providers.market.share")}</option>
              <option value="client">{t("providers.market.client")}</option>
            </select>
          </label>
          <label className="grid min-w-0 gap-1 text-xs font-medium text-slate-600">
            <span>{t("providers.rankFilter.label")}</span>
            <select
              value={rankFilter}
              onChange={(event) => setRankFilter(event.target.value as ProviderRankFilter)}
              disabled={providerLoading && !providerList}
              className="min-h-11 w-full rounded-lg border border-slate-200 bg-white px-3 text-sm text-slate-950 outline-2 outline-offset-1 outline-transparent hover:bg-slate-50 active:bg-slate-100 focus-visible:outline-primary disabled:cursor-not-allowed disabled:bg-slate-50 disabled:text-slate-400 disabled:opacity-[0.55]"
            >
              <option value="all">{t("providers.rankFilter.all")}</option>
              <option value="ranked">{t("providers.rankFilter.ranked")}</option>
              <option value="collecting">{t("providers.rankFilter.collecting")}</option>
            </select>
          </label>
          <Button
            variant="ghost"
            size="sm"
            className="min-h-11 self-end whitespace-nowrap"
            isDisabled={!filtersActive}
            onClick={resetFilters}
          >
            {t("providers.filters.clear")}
          </Button>
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

      <section className="grid min-w-0 gap-4" aria-labelledby="provider-funding-title">
        <div className="flex min-w-0 flex-wrap items-end justify-between gap-3">
          <div>
            <h2 id="provider-funding-title" className="flex items-center gap-2 text-base font-semibold text-slate-950">
              <WalletCards className="h-4 w-4 text-slate-500" aria-hidden="true" />
              {t("providers.funding.title")}
            </h2>
            <p className="mt-1 max-w-3xl text-xs leading-5 text-slate-500">{t("providers.funding.subtitle")}</p>
          </div>
          {authed ? (
            <Button
              variant="ghost"
              size="sm"
              className="min-h-11 whitespace-nowrap"
              isDisabled={fundingLoading}
              onClick={() => void loadFunding()}
            >
              {fundingLoading ? <Loader2 className="h-4 w-4 animate-spin" aria-hidden="true" /> : <RefreshCw className="h-4 w-4" aria-hidden="true" />}
              {t("common.reload")}
            </Button>
          ) : null}
        </div>

        {!authLoading && !authed ? (
          <div className="flex min-w-0 flex-wrap items-center justify-between gap-3 rounded-xl border border-slate-200 bg-white px-4 py-4">
            <div className="min-w-0">
              <strong className="block text-sm text-slate-950">{t("providers.funding.loginTitle")}</strong>
              <span className="mt-1 block text-sm text-slate-600">{t("providers.funding.loginPrompt")}</span>
            </div>
            <Button variant="primary" className="min-h-11 shrink-0 whitespace-nowrap" onClick={openLogin}>
              {t("providers.funding.loginAction")}
            </Button>
          </div>
        ) : null}

        {authed ? (
          <div className="min-w-0 overflow-hidden rounded-xl border border-slate-200 bg-white" aria-busy={fundingLoading}>
            {funding ? <FundingTotals funding={funding} /> : null}
            {fundingLoading && !funding ? <LoadingRows /> : null}
            {fundingError ? (
              <div className="grid min-h-32 place-items-center gap-3 px-4 py-8 text-center">
                <p role="alert" className="max-w-xl text-sm text-rose-700">{fundingError}</p>
                <Button variant="outline" size="sm" className="min-h-11 whitespace-nowrap" onClick={() => void loadFunding()}>{t("common.retry")}</Button>
              </div>
            ) : null}
            {!fundingLoading && !fundingError && funding?.providers.length === 0 ? (
              <div className="grid min-h-32 place-items-center px-4 py-8 text-center text-sm text-slate-500">
                {t("providers.funding.empty")}
              </div>
            ) : null}
            {funding?.providers.map((provider) => (
              <FundingRow key={provider.marketProviderId} provider={provider} onTopup={setTopupProvider} />
            ))}
          </div>
        ) : null}
      </section>

      <ProviderDetailDrawer
        providerId={providerId}
        summary={selectedSummary}
        detail={detail}
        loading={detailLoading}
        error={detailError}
        onClose={() => updateProviderQuery()}
      />

      <MarketFundingTopupDialog
        open={!!visibleTopupProvider}
        funding={visibleTopupProvider ? preferredProviderTopupFunding(visibleTopupProvider) : undefined}
        onClose={() => setTopupProvider(null)}
        onCredited={handleCredited}
      />
    </main>
  );
}
