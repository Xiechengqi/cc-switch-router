"use client";

/* Hallmark · genre: modern-minimal · macrostructure: Workbench · theme: existing router system · enrichment: none · nav: existing app shell · footer: none
 * audience: Router market users · task: compare Provider trust, then open a market · tone: technical
 * The list is a published ranking, so every row says where its number comes
 * from: a formal rank with its score, or how far a collecting Provider is
 * from earning one. Market, availability and the open Provider live in the
 * URL; the search text does not, like every other market search.
 */

import * as React from "react";
import Link from "next/link";
import { useSearchParams } from "next/navigation";
import { Button, Drawer } from "@heroui/react";
import {
  AlertTriangle,
  ArrowUpRight,
  BarChart3,
  ChevronRight,
  Clock,
  Gauge,
  Loader2,
  RefreshCw,
  Search,
} from "lucide-react";
import { CountryFlag } from "@/components/common/country-flag";
import { SegmentedControl } from "@/components/common/segmented-control";
import {
  ProviderBadges,
  RankMark,
  RankProgressChips,
  useProviderName,
} from "@/components/dashboard/provider-rank-parts";
import { drawerDialogClassName } from "@/components/dashboard/share-dashboard-utils";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { getMarketProvider, getMarketProviders } from "@/lib/api";
import { rememberedView, rememberView } from "@/lib/cached-resource";
import { clientMarketHref, shareMarketHref } from "@/lib/dashboard-nav";
import type {
  MarketProvider,
  MarketProviderDetail,
  MarketProviderList,
  MarketProviderRankGeneration,
} from "@/lib/types";
import { replaceCurrentSearch } from "@/lib/url-search";
import { cn, formatRelativeTime } from "@/lib/utils";
import {
  actorBoundValue,
  filterMarketProviders,
  formatBasisPointScore,
  isFormallyRanked,
  providerLoadError,
  providerMarketFilter,
  type ProviderLoadError,
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

/** Re-renders once a minute so relative times stay true while the page is open. */
function useMinuteTick() {
  const [, setTick] = React.useState(0);
  React.useEffect(() => {
    const timer = window.setInterval(() => setTick((value) => value + 1), 60_000);
    return () => window.clearInterval(timer);
  }, []);
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
    <dl className="grid min-w-0 grid-cols-2 gap-x-5 gap-y-3 sm:grid-cols-4">
      {values.map(([label, value]) => (
        <div key={label} className="grid min-w-0 gap-1">
          <dt className="truncate text-[11px] leading-4 text-muted-foreground" title={label}>{label}</dt>
          <dd className="font-mono text-xs font-semibold tabular-nums text-foreground">
            {formatBasisPointScore(value, locale)}
          </dd>
          <span className="h-1 overflow-hidden rounded-full bg-muted" aria-hidden="true">
            <span
              className="block h-full rounded-full bg-gradient-to-r from-accent to-accent-secondary"
              style={{ width: `${Math.max(0, Math.min(10_000, value)) / 100}%` }}
            />
          </span>
        </div>
      ))}
    </dl>
  );
}

function SupplySummary({ provider }: { provider: MarketProvider }) {
  const { t } = useLocaleText();
  const supply = [
    provider.inventory.hasShareMarket
      ? { key: "share", available: provider.inventory.availableShareSeats, label: t("providers.supply.shareAvailable", { count: provider.inventory.availableShareSeats }) }
      : null,
    provider.inventory.hasClientMarket
      ? { key: "client", available: provider.inventory.idleHostTotal, label: t("providers.supply.clientAvailable", { count: provider.inventory.idleHostTotal }) }
      : null,
  ].filter((item): item is { key: string; available: number; label: string } => !!item);
  return (
    <span className="flex min-w-0 flex-wrap gap-x-3 gap-y-1 text-xs text-muted-foreground">
      {supply.length ? supply.map((item) => (
        <span key={item.key} className="inline-flex items-center gap-1.5 whitespace-nowrap">
          <span
            className={cn("h-1.5 w-1.5 rounded-full", item.available > 0 ? "bg-emerald-500" : "bg-slate-300")}
            aria-hidden="true"
          />
          {item.label}
        </span>
      )) : (
        <span>{t("providers.supply.noneAvailable")}</span>
      )}
    </span>
  );
}

function ProviderRankRow({
  provider,
  generation,
  onOpen,
}: {
  provider: MarketProvider;
  generation: MarketProviderRankGeneration;
  onOpen: (providerId: string) => void;
}) {
  const { locale, t } = useLocaleText();
  const nameOf = useProviderName();
  const ranked = isFormallyRanked(provider);
  return (
    <button
      type="button"
      className={cn(
        "grid min-h-11 w-full min-w-0 grid-cols-[minmax(0,1fr)_auto] gap-x-4 gap-y-3 px-4 py-4 text-left outline-none transition-colors hover:bg-muted/60 active:bg-muted focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring sm:grid-cols-[4.5rem_minmax(0,1fr)_auto] sm:items-center sm:px-5",
        provider.status === "paused" && "opacity-75",
      )}
      onClick={() => onOpen(provider.id)}
    >
      <span className="col-start-1 row-start-1 self-start sm:self-center">
        <RankMark provider={provider} />
      </span>
      <span className="col-span-2 col-start-1 row-start-2 grid min-w-0 gap-2 sm:col-span-1 sm:col-start-2 sm:row-start-1">
        <span className="flex min-w-0 flex-wrap items-center gap-2">
          <strong className="min-w-0 text-sm font-semibold text-foreground [overflow-wrap:anywhere]">
            {nameOf(provider)}
          </strong>
          <ProviderBadges provider={provider} />
        </span>
        {ranked ? (
          <span className="flex min-w-0 flex-wrap gap-x-3 gap-y-1 text-xs text-muted-foreground">
            <span>
              {t("providers.component.serviceQuality")}{" "}
              <strong className="font-mono font-semibold tabular-nums text-foreground">
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
          <RankProgressChips provider={provider} generation={generation} />
        )}
        <SupplySummary provider={provider} />
      </span>
      <span className="col-start-2 row-start-1 flex shrink-0 items-center justify-end gap-3 sm:col-start-3">
        {ranked ? (
          <span className="text-right">
            <span className="block text-[11px] text-muted-foreground">{t("providers.score")}</span>
            <strong className="mt-0.5 block font-mono text-base font-semibold tabular-nums text-foreground sm:text-lg">
              {formatBasisPointScore(provider.scoreBps, locale)}
            </strong>
          </span>
        ) : null}
        <span className="hidden items-center gap-1 whitespace-nowrap text-xs font-semibold text-primary md:inline-flex">
          {t("providers.viewSupply")}
          <ChevronRight className="h-4 w-4" aria-hidden="true" />
        </span>
        <ChevronRight className="h-4 w-4 text-muted-foreground md:hidden" aria-hidden="true" />
      </span>
    </button>
  );
}

function LoadingRows() {
  return (
    <div className="grid" aria-hidden="true">
      {[0, 1, 2].map((index) => (
        <div key={index} className="grid gap-3 border-t border-border/60 px-4 py-5 first:border-t-0">
          <div className="h-4 w-40 animate-pulse rounded bg-muted" />
          <div className="h-3 w-full max-w-xl animate-pulse rounded bg-muted" />
        </div>
      ))}
    </div>
  );
}

function StatePanel({
  icon: Icon,
  title,
  body,
  tone = "neutral",
  action,
}: {
  icon: React.ComponentType<{ className?: string; "aria-hidden"?: boolean | "true" }>;
  title: string;
  body?: string;
  tone?: "neutral" | "danger";
  action?: React.ReactNode;
}) {
  return (
    <div className="grid min-h-40 place-items-center px-4 py-10 text-center" role={tone === "danger" ? "alert" : "status"}>
      <div className="grid max-w-md justify-items-center gap-2">
        <span className={cn("grid h-10 w-10 place-items-center rounded-full", tone === "danger" ? "bg-rose-50 text-rose-700" : "bg-primary/10 text-primary")}>
          <Icon className="h-5 w-5" aria-hidden="true" />
        </span>
        <p className="text-sm font-semibold text-foreground">{title}</p>
        {body ? <p className="text-sm leading-6 text-muted-foreground">{body}</p> : null}
        {action}
      </div>
    </div>
  );
}

function DrawerShell({
  open,
  width,
  onClose,
  children,
}: {
  open: boolean;
  width: "wide" | "narrow";
  onClose: () => void;
  children: React.ReactNode;
}) {
  return (
    <Drawer.Backdrop isOpen={open} onOpenChange={(next) => !next && onClose()}>
      <Drawer.Content placement="right">
        <Drawer.Dialog
          className={cn(
            drawerDialogClassName,
            "h-full",
            width === "wide" ? "!w-[min(680px,calc(100vw-0.5rem))]" : "!w-[min(520px,calc(100vw-0.5rem))]",
          )}
        >
          <Drawer.CloseTrigger className="!h-11 !w-11 !bg-slate-100 !text-slate-700 hover:!bg-slate-200 active:!bg-slate-300" />
          {children}
        </Drawer.Dialog>
      </Drawer.Content>
    </Drawer.Backdrop>
  );
}

function SupplyHeading({ title, href, linkLabel }: { title: string; href?: string; linkLabel: string }) {
  const { t } = useLocaleText();
  return (
    <div className="flex min-w-0 flex-wrap items-center justify-between gap-3">
      <h3 className="flex items-center gap-2 text-sm font-semibold">
        {title}
        <span className="inline-flex items-center gap-1 rounded-full bg-emerald-50 px-1.5 py-0.5 text-[10px] font-semibold uppercase tracking-wide text-emerald-700">
          <span className="h-1.5 w-1.5 rounded-full bg-emerald-500 motion-safe:animate-pulse" aria-hidden="true" />
          {t("providers.detail.live")}
        </span>
      </h3>
      {href ? (
        <Link
          href={href}
          className="inline-flex min-h-11 shrink-0 items-center gap-1 whitespace-nowrap text-sm font-medium text-primary outline-none hover:underline active:text-primary/70 focus-visible:ring-2 focus-visible:ring-ring"
        >
          {linkLabel}
          <ArrowUpRight className="h-4 w-4" aria-hidden="true" />
        </Link>
      ) : null}
    </div>
  );
}

function ProviderDetailDrawer({
  providerId,
  summary,
  generation,
  detail,
  loading,
  error,
  onClose,
}: {
  providerId: string;
  summary?: MarketProvider;
  generation: MarketProviderRankGeneration | null;
  detail: MarketProviderDetail | null;
  loading: boolean;
  error: ProviderLoadError | "";
  onClose: () => void;
}) {
  const { locale, t } = useLocaleText();
  const nameOf = useProviderName();
  const provider = detail ?? summary;
  const ranked = provider ? isFormallyRanked(provider) : false;
  const regionNames = React.useMemo(() => {
    try {
      return new Intl.DisplayNames([locale], { type: "region" });
    } catch {
      return null;
    }
  }, [locale]);
  const performance = provider?.performance;
  const hasPerformance = performance?.averageTtftMs != null || performance?.averageTps != null;
  return (
    <DrawerShell open={!!providerId} width="wide" onClose={onClose}>
      <Drawer.Header>
        <div className="min-w-0 pr-10">
          <Drawer.Heading className="min-w-0 text-lg [overflow-wrap:anywhere]">
            {provider ? nameOf(provider) : t("providers.detail.title")}
          </Drawer.Heading>
          {provider ? <div className="mt-2"><ProviderBadges provider={provider} /></div> : null}
        </div>
      </Drawer.Header>
      <Drawer.Body className="grid content-start gap-6 overflow-y-auto pb-8">
        {loading && !provider ? (
          <div className="flex min-h-40 items-center justify-center gap-2 text-sm text-muted-foreground">
            <Loader2 className="h-4 w-4 animate-spin" aria-hidden="true" />
            {t("common.loading")}
          </div>
        ) : null}
        {error ? (
          <p role="alert" className="rounded-lg bg-rose-50 px-3 py-2 text-sm text-rose-800">
            {t(error === "notFound" ? "providers.error.notFound" : "providers.error.detail")}
          </p>
        ) : null}
        {provider ? (
          <>
            <section className="grid gap-4">
              {ranked ? (
                <>
                  <div className="flex min-w-0 flex-wrap items-end justify-between gap-3">
                    <div>
                      <span className="block text-xs text-muted-foreground">{t("providers.score")}</span>
                      <strong className="mt-1 block font-mono text-3xl font-semibold tabular-nums">
                        {formatBasisPointScore(provider.scoreBps, locale)}
                      </strong>
                    </div>
                    <RankMark provider={provider} />
                  </div>
                  <ScoreComponents provider={provider} />
                </>
              ) : (
                <div className="grid gap-2 rounded-lg border border-dashed border-border px-3 py-3">
                  <div className="flex min-w-0 flex-wrap items-center justify-between gap-2">
                    <span className="text-sm font-semibold">{t("providers.progress.label")}</span>
                    <RankMark provider={provider} />
                  </div>
                  {generation ? <RankProgressChips provider={provider} generation={generation} /> : null}
                  <p className="text-xs leading-5 text-muted-foreground">{t("providers.component.unranked")}</p>
                </div>
              )}
              <dl className="grid grid-cols-2 gap-3 text-sm sm:grid-cols-4">
                <div><dt className="text-xs text-muted-foreground">{t("providers.independentBuyers")}</dt><dd className="mt-1 font-mono font-semibold tabular-nums">{provider.independentBuyerCount}</dd></div>
                <div><dt className="text-xs text-muted-foreground">{t("providers.observationDays")}</dt><dd className="mt-1 font-mono font-semibold tabular-nums">{provider.observationDays}</dd></div>
                <div><dt className="text-xs text-muted-foreground">{t("providers.quality")}</dt><dd className="mt-1 font-mono font-semibold tabular-nums">{provider.quality.fulfillmentSuccesses}/{provider.quality.fulfillmentTotal}</dd></div>
                <div><dt className="text-xs text-muted-foreground">{t("providers.joinedAt")}</dt><dd className="mt-1 font-medium">{formatDate(provider.joinedAt, locale)}</dd></div>
              </dl>
            </section>

            {hasPerformance && performance ? (
              <section className="grid gap-3 rounded-lg bg-muted/60 px-3 py-3">
                <div className="flex items-center gap-2">
                  <Gauge className="h-4 w-4 text-muted-foreground" aria-hidden="true" />
                  <h3 className="text-sm font-semibold">{t("providers.performance.title")}</h3>
                </div>
                <dl className="grid grid-cols-2 gap-3">
                  <div><dt className="text-xs text-muted-foreground">TTFT</dt><dd className="mt-1 font-mono text-sm font-semibold tabular-nums">{performance.averageTtftMs == null ? "—" : `${Math.round(performance.averageTtftMs)} ms`}</dd></div>
                  <div><dt className="text-xs text-muted-foreground">TPS</dt><dd className="mt-1 font-mono text-sm font-semibold tabular-nums">{performance.averageTps == null ? "—" : performance.averageTps.toFixed(1)}</dd></div>
                </dl>
                <p className="text-xs leading-5 text-muted-foreground">
                  {performance.sampleCount && generation?.windows
                    ? `${t("providers.performance.samples", { count: performance.sampleCount, days: generation.windows.performanceDays })} `
                    : ""}
                  {t("providers.performance.displayOnly")}
                </p>
              </section>
            ) : null}

            {detail ? (
              <section className="grid gap-5">
                <div className="grid gap-2">
                  <SupplyHeading
                    title={t("providers.detail.shareSupply")}
                    href={detail.inventory.hasShareMarket ? shareMarketHref({ providerId: detail.id }) : undefined}
                    linkLabel={t("providers.viewShareMarket")}
                  />
                  {detail.shareSupply.length ? (
                    <div className="grid gap-1">
                      {detail.shareSupply.map((supply) => {
                        const body = (
                          <>
                            <span className="min-w-0">
                              <strong className="block truncate">{supply.shareName}</strong>
                              <span className="block truncate text-xs text-muted-foreground">{supply.apps.join(" · ") || "—"}</span>
                            </span>
                            <span className="flex items-center gap-2 whitespace-nowrap font-mono text-xs tabular-nums text-muted-foreground">
                              {t("providers.detail.seatsValue", { available: supply.availableSeats, total: supply.totalSeats })}
                              {supply.shareId ? <ChevronRight className="h-4 w-4" aria-hidden="true" /> : null}
                            </span>
                          </>
                        );
                        const rowClass = "grid min-h-11 min-w-0 gap-1 rounded-md bg-muted/60 px-3 py-2 text-sm sm:grid-cols-[minmax(0,1fr)_auto] sm:items-center sm:gap-3";
                        return supply.shareId ? (
                          <Link
                            key={supply.listingId}
                            href={shareMarketHref({ shareId: supply.shareId, providerId: detail.id })}
                            aria-label={t("providers.detail.openShare", { name: supply.shareName })}
                            className={cn(rowClass, "outline-none transition-colors hover:bg-muted focus-visible:ring-2 focus-visible:ring-ring")}
                          >
                            {body}
                          </Link>
                        ) : (
                          <div key={supply.listingId} className={rowClass}>{body}</div>
                        );
                      })}
                    </div>
                  ) : <p className="text-sm text-muted-foreground">{t("providers.detail.noShareSupply")}</p>}
                </div>

                <div className="grid gap-2">
                  <SupplyHeading
                    title={t("providers.detail.clientSupply")}
                    href={detail.inventory.hasClientMarket ? clientMarketHref({ providerId: detail.id }) : undefined}
                    linkLabel={t("providers.viewClientMarket")}
                  />
                  {detail.clientSupply.length ? (
                    <div className="grid gap-1 sm:grid-cols-2">
                      {detail.clientSupply.map((supply) => {
                        const known = supply.countryCode.length === 2;
                        const name = known ? regionNames?.of(supply.countryCode) || supply.countryCode : supply.countryCode;
                        return (
                          <div key={supply.countryCode} className="grid min-w-0 grid-cols-[minmax(0,1fr)_auto] items-center gap-3 rounded-md bg-muted/60 px-3 py-2 text-sm">
                            <span className="flex min-w-0 items-center gap-2">
                              {known ? <CountryFlag code={supply.countryCode} title={name} /> : null}
                              <strong className="truncate">{name}</strong>
                            </span>
                            <span className="whitespace-nowrap font-mono text-xs tabular-nums text-muted-foreground">{t("providers.detail.hostsValue", { idle: supply.idleHosts, total: supply.totalHosts })}</span>
                          </div>
                        );
                      })}
                    </div>
                  ) : <p className="text-sm text-muted-foreground">{t("providers.detail.noClientSupply")}</p>}
                </div>
              </section>
            ) : null}
          </>
        ) : null}
      </Drawer.Body>
    </DrawerShell>
  );
}

function RankingMethodDrawer({
  open,
  generation,
  onClose,
}: {
  open: boolean;
  generation: MarketProviderRankGeneration | null;
  onClose: () => void;
}) {
  const { locale, t } = useLocaleText();
  const components = generation ? [
    [t("providers.component.serviceQuality"), generation.weights.serviceQuality],
    [t("providers.component.effectiveChoice"), generation.weights.effectiveChoice],
    [t("providers.component.fulfillment"), generation.weights.fulfillment],
    [t("providers.component.supplyBreadth"), generation.weights.supplyBreadth],
  ] as const : [];
  const windows = generation?.windows;
  return (
    <DrawerShell open={open} width="narrow" onClose={onClose}>
      <Drawer.Header>
        <Drawer.Heading className="pr-10 text-lg">
          {t("providers.method.title")}
        </Drawer.Heading>
      </Drawer.Header>
      <Drawer.Body className="grid content-start gap-6 overflow-y-auto pb-8">
        {!generation ? (
          <div className="flex min-h-40 items-center justify-center gap-2 text-sm text-muted-foreground">
            <Loader2 className="h-4 w-4 animate-spin" aria-hidden="true" />
            {t("common.loading")}
          </div>
        ) : (
          <>
            <p className="text-sm leading-6 text-muted-foreground">
              {t("providers.method.summary")}
            </p>

            <section className="grid gap-3" aria-labelledby="provider-method-weights">
              <h3 id="provider-method-weights" className="text-sm font-semibold text-foreground">
                {t("providers.method.weights")}
              </h3>
              <dl className="grid grid-cols-2 gap-2">
                {components.map(([label, weight]) => (
                  <div key={label} className="grid gap-2 rounded-lg bg-muted/60 px-3 py-3">
                    <dt className="text-xs text-muted-foreground">{label}</dt>
                    <dd className="font-mono text-lg font-semibold tabular-nums text-foreground">
                      {t("providers.method.weightValue", { weight })}
                    </dd>
                    <span className="h-1 overflow-hidden rounded-full bg-background" aria-hidden="true">
                      <span className="block h-full rounded-full bg-gradient-to-r from-accent to-accent-secondary" style={{ width: `${weight}%` }} />
                    </span>
                  </div>
                ))}
              </dl>
            </section>

            <section className="grid gap-2" aria-labelledby="provider-method-eligibility">
              <h3 id="provider-method-eligibility" className="text-sm font-semibold text-foreground">
                {t("providers.method.eligibility")}
              </h3>
              <p className="text-sm leading-6 text-muted-foreground">
                {t("providers.method.eligibilityText", {
                  buyers: generation.minimumIndependentBuyers,
                  days: generation.minimumObservationDays,
                })}
              </p>
            </section>

            {windows ? (
              <section className="grid gap-2" aria-labelledby="provider-method-windows">
                <h3 id="provider-method-windows" className="text-sm font-semibold text-foreground">
                  {t("providers.method.windows")}
                </h3>
                <ul className="grid list-disc gap-1.5 pl-5 text-sm leading-6 text-muted-foreground">
                  <li>{t("providers.method.window.serviceQuality", { shareDays: windows.shareQualityDays, hostDays: windows.hostQualityDays })}</li>
                  <li>{t("providers.method.window.effectiveChoice", { days: windows.effectiveChoiceDays })}</li>
                  <li>{t("providers.method.window.fulfillment", { days: windows.fulfillmentDays })}</li>
                  <li>{t("providers.method.window.performance", { days: windows.performanceDays })}</li>
                  {generation.refreshIntervalSecs ? (
                    <li>{t("providers.method.refresh", { minutes: Math.round(generation.refreshIntervalSecs / 60) })}</li>
                  ) : null}
                </ul>
              </section>
            ) : null}

            <section className="grid gap-2 rounded-lg bg-muted/60 px-3 py-3" aria-labelledby="provider-method-notes">
              <h3 id="provider-method-notes" className="text-sm font-semibold text-foreground">
                {t("providers.method.notes")}
              </h3>
              <ul className="grid list-disc gap-2 pl-5 text-sm leading-6 text-muted-foreground">
                <li>{t("providers.method.officialNote")}</li>
                <li>{t("providers.method.explorationNote")}</li>
                <li>{t("providers.method.filterNote")}</li>
                <li>{t("providers.method.performanceNote")}</li>
              </ul>
            </section>

            <dl className="grid gap-2 text-xs text-muted-foreground">
              <div className="flex min-w-0 items-baseline justify-between gap-4">
                <dt>{t("providers.method.published")}</dt>
                <dd className="text-right text-foreground">
                  {formatDateTime(generation.publishedAt, locale)}
                </dd>
              </div>
              <div className="flex min-w-0 items-baseline justify-between gap-4">
                <dt>{t("providers.method.version")}</dt>
                <dd className="min-w-0 break-all text-right font-mono text-foreground">
                  {generation.algorithmVersion}
                </dd>
              </div>
            </dl>
          </>
        )}
      </Drawer.Body>
    </DrawerShell>
  );
}

const PROVIDER_LIST_VIEW_KEY = "market-providers";

export function ProvidersPage() {
  const { locale, t } = useLocaleText();
  const nameOf = useProviderName();
  useMinuteTick();
  const searchParams = useSearchParams();
  const providerId = searchParams.get("provider") || "";
  const marketFilter = providerMarketFilter(searchParams.get("market"));
  const availableOnly = searchParams.get("available") === "1";

  // The public ranking needs no account scope; a remembered list renders at
  // once while it is re-read.
  const [providerList, setProviderList] = React.useState<MarketProviderList | null>(
    () => rememberedView<MarketProviderList>(PROVIDER_LIST_VIEW_KEY) ?? null,
  );
  const [providerLoading, setProviderLoading] = React.useState(true);
  const [providerError, setProviderError] = React.useState<ProviderLoadError | null>(null);
  const [detailSnapshot, setDetailSnapshot] = React.useState<{
    actorKey: string;
    value: MarketProviderDetail;
  } | null>(null);
  const [detailLoadingFor, setDetailLoadingFor] = React.useState<string | null>(null);
  const [detailErrorSnapshot, setDetailErrorSnapshot] = React.useState<{
    actorKey: string;
    value: ProviderLoadError;
  } | null>(null);
  const [methodOpen, setMethodOpen] = React.useState(false);
  const [query, setQuery] = React.useState("");
  const listAbortRef = React.useRef<AbortController | null>(null);
  const detail = actorBoundValue(detailSnapshot, providerId);
  const detailError = actorBoundValue(detailErrorSnapshot, providerId) || "";
  const detailLoading = detailLoadingFor === providerId;

  const loadProviders = React.useCallback(async () => {
    listAbortRef.current?.abort();
    const controller = new AbortController();
    listAbortRef.current = controller;
    setProviderLoading(true);
    setProviderError(null);
    try {
      const next = await getMarketProviders(controller.signal);
      if (!controller.signal.aborted) {
        setProviderList(next);
        rememberView(PROVIDER_LIST_VIEW_KEY, next);
      }
    } catch (reason) {
      if (!controller.signal.aborted) setProviderError(providerLoadError(reason));
    } finally {
      if (listAbortRef.current === controller) {
        listAbortRef.current = null;
        setProviderLoading(false);
      }
    }
  }, []);

  React.useEffect(() => {
    void loadProviders();
    return () => listAbortRef.current?.abort();
  }, [loadProviders]);

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
          setDetailErrorSnapshot({ actorKey: providerId, value: providerLoadError(reason) });
        }
      })
      .finally(() => {
        if (!controller.signal.aborted) setDetailLoadingFor(null);
      });
    return () => controller.abort();
  }, [providerId]);

  const updateProviderQuery = React.useCallback((nextProviderId?: string) => {
    replaceCurrentSearch((params) => {
      if (nextProviderId) params.set("provider", nextProviderId);
      else params.delete("provider");
    });
  }, []);
  const setMarketFilter = React.useCallback((next: ProviderMarketFilter) => {
    replaceCurrentSearch((params) => {
      if (next === "all") params.delete("market");
      else params.set("market", next);
    });
  }, []);
  const setAvailableOnly = React.useCallback((next: boolean) => {
    replaceCurrentSearch((params) => {
      if (next) params.set("available", "1");
      else params.delete("available");
    });
  }, []);

  const generation = providerList?.generation ?? null;
  const providers = providerList?.providers;
  const visibleProviders = React.useMemo(
    () => filterMarketProviders(providers ?? [], {
      query,
      market: marketFilter,
      availableOnly,
      nameOf,
    }),
    [availableOnly, marketFilter, nameOf, providers, query],
  );
  const selectedSummary = providers?.find((provider) => provider.id === providerId);
  const filtersActive = !!query || marketFilter !== "all" || availableOnly;
  const noneRanked = !!providers?.length && !providers.some(isFormallyRanked);
  const waiting = providerLoading && !providerList;

  const resetFilters = () => {
    setQuery("");
    replaceCurrentSearch((params) => {
      params.delete("market");
      params.delete("available");
    });
  };

  return (
    <main className="mx-auto grid w-[calc(100%-2rem)] min-w-0 max-w-7xl gap-8 pb-12">
      <header className="flex min-w-0 justify-end">
        <h1 className="sr-only">{t("providers.title")}</h1>
        <div className="flex min-w-0 flex-wrap items-center justify-end gap-2">
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

      {generation?.stale ? (
        <div className="flex min-w-0 items-start gap-2 rounded-lg bg-amber-50 px-3 py-2 text-sm text-amber-900" role="status">
          <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0" aria-hidden="true" />
          <span>{t("providers.generation.stale", { time: formatDateTime(generation.publishedAt, locale) })}</span>
        </div>
      ) : null}
      {providerError && providerList ? (
        <div className="flex min-w-0 flex-wrap items-center justify-between gap-2 rounded-lg bg-rose-50 px-3 py-2 text-sm text-rose-800" role="alert">
          <span>{t("providers.error.load")}</span>
          <Button variant="ghost" size="sm" className="min-h-11 whitespace-nowrap" onClick={() => void loadProviders()}>
            {t("common.retry")}
          </Button>
        </div>
      ) : null}

      {noneRanked && generation ? (
        <section className="relative grid min-w-0 gap-1 overflow-hidden rounded-xl border border-primary/20 bg-primary/5 px-5 py-5 text-foreground">
          <span className="pointer-events-none absolute -right-16 -top-16 h-48 w-48 rounded-full bg-accent/10 blur-3xl" aria-hidden="true" />
          <h2 className="relative text-base font-semibold">{t("providers.noRanked.title")}</h2>
          <p className="relative max-w-3xl text-sm leading-6 text-muted-foreground">
            {t("providers.noRanked.body", {
              buyers: generation.minimumIndependentBuyers,
              days: generation.minimumObservationDays,
            })}
          </p>
        </section>
      ) : null}

      <section className="grid min-w-0 gap-4" aria-labelledby="provider-ranking-title">
        <div className="flex min-w-0 flex-wrap items-baseline justify-between gap-3">
          <h2 id="provider-ranking-title" className="text-base font-semibold text-foreground">
            {t("providers.rankingTitle")}
          </h2>
          <span className="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1 text-xs tabular-nums text-muted-foreground">
            {providerList ? <span>{t("providers.results", { count: visibleProviders.length })}</span> : null}
            {generation ? (
              <span className="inline-flex items-center gap-1" title={formatDateTime(generation.publishedAt, locale)}>
                <Clock className="h-3.5 w-3.5" aria-hidden="true" />
                {t("providers.updatedAt", { time: formatRelativeTime(generation.publishedAt, locale) })}
              </span>
            ) : null}
          </span>
        </div>

        <div className="flex min-w-0 flex-col gap-3 lg:flex-row lg:items-center">
          <label className="relative min-w-0 flex-1 lg:max-w-md">
            <span className="sr-only">{t("providers.search.label")}</span>
            <Search className="pointer-events-none absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" aria-hidden="true" />
            <input
              type="search"
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              placeholder={t("providers.search.placeholder")}
              disabled={waiting}
              className="min-h-11 w-full rounded-lg border border-border bg-card py-2 pl-9 pr-3 text-sm text-foreground outline-2 outline-offset-1 outline-transparent placeholder:text-muted-foreground/70 hover:bg-muted/40 focus-visible:outline-ring disabled:cursor-not-allowed disabled:bg-muted disabled:opacity-[0.55]"
            />
          </label>
          <div className="flex min-w-0 flex-wrap items-center gap-2">
            <SegmentedControl
              value={marketFilter}
              onChange={setMarketFilter}
              ariaLabel={t("providers.market.label")}
              size="md"
              disabled={waiting}
              items={[
                { id: "all", label: t("providers.market.all") },
                { id: "share", label: t("providers.market.share") },
                { id: "client", label: t("providers.market.client") },
              ]}
            />
            <button
              type="button"
              aria-pressed={availableOnly}
              disabled={waiting}
              onClick={() => setAvailableOnly(!availableOnly)}
              className={cn(
                "inline-flex min-h-11 items-center gap-2 whitespace-nowrap rounded-lg border px-3 text-sm font-medium transition-colors outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-[0.55]",
                availableOnly
                  ? "border-primary/40 bg-primary/10 text-primary"
                  : "border-border bg-card text-muted-foreground hover:text-foreground",
              )}
            >
              <span className={cn("h-2 w-2 rounded-full", availableOnly ? "bg-emerald-500" : "bg-slate-300")} aria-hidden="true" />
              {t("providers.availableOnly")}
            </button>
            {filtersActive ? (
              <Button variant="ghost" size="sm" className="min-h-11 whitespace-nowrap" onClick={resetFilters}>
                {t("providers.filters.clear")}
              </Button>
            ) : null}
          </div>
        </div>

        <div className="min-w-0 overflow-hidden rounded-xl border border-border bg-card shadow-[0_1px_3px_rgba(0,0,0,0.06)]" aria-busy={providerLoading}>
          {waiting ? <LoadingRows /> : null}
          {providerError && !providerList ? (
            providerError === "notReady" ? (
              <StatePanel
                icon={Clock}
                title={t("providers.notReady.title")}
                body={t("providers.notReady.body")}
                action={(
                  <Button variant="outline" size="sm" className="mt-2 min-h-11 whitespace-nowrap" onClick={() => void loadProviders()}>
                    {t("common.reload")}
                  </Button>
                )}
              />
            ) : (
              <StatePanel
                icon={AlertTriangle}
                tone="danger"
                title={t("providers.error.load")}
                action={(
                  <Button variant="outline" size="sm" className="mt-2 min-h-11 whitespace-nowrap" onClick={() => void loadProviders()}>
                    {t("common.retry")}
                  </Button>
                )}
              />
            )
          ) : null}
          {providerList && generation && visibleProviders.length === 0 ? (
            <StatePanel
              icon={Search}
              title={filtersActive ? t("providers.empty.filtered") : t("providers.empty.all")}
              action={filtersActive ? (
                <Button variant="ghost" size="sm" className="mt-2 min-h-11 whitespace-nowrap" onClick={resetFilters}>
                  {t("providers.filters.clear")}
                </Button>
              ) : undefined}
            />
          ) : null}
          {generation ? visibleProviders.map((provider, index) => (
            <div key={provider.id} className={index ? "border-t border-border/60" : undefined}>
              <ProviderRankRow provider={provider} generation={generation} onOpen={updateProviderQuery} />
            </div>
          )) : null}
        </div>
      </section>

      <ProviderDetailDrawer
        providerId={providerId}
        summary={selectedSummary}
        generation={generation}
        detail={detail}
        loading={detailLoading}
        error={detailError}
        onClose={() => updateProviderQuery()}
      />

      <RankingMethodDrawer
        open={methodOpen}
        generation={generation}
        onClose={() => setMethodOpen(false)}
      />
    </main>
  );
}
