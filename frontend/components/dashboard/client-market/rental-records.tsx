"use client";

import { LeaseActions } from "./lease-actions";
import * as React from "react";
import Link from "next/link";
import { Button } from "@heroui/react";
import { ChevronDown, ChevronLeft, ChevronRight, Loader2, MessageCircle, TriangleAlert } from "lucide-react";
import { useAuth } from "@/components/auth/auth-provider";
import { useClientChat } from "@/components/chat/client-chat";
import { ExpandableRecordTable, RecordDetailList, type RecordTableColumn } from "@/components/common/expandable-record-table";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { ClientTerminationDialog } from "@/components/market/client-termination-dialog";
import { MarketWalletRenewalCard } from "@/components/dashboard/market-wallet-renewal-card";
import { MarketServiceDisputeControl } from "@/components/dashboard/market-service-dispute-control";
import { ApiError } from "@/lib/api";
import { clientRentalLabel, getClientRentals, getClientRentalDetail, CLIENT_RENTAL_MONEY_FIELDS,
  type ClientRentalRecord, type ClientRentalPage, type ClientRentalView, type ClientRentalDetail } from "@/lib/client-rentals";
import { parseAuditPageNumber } from "@/lib/client-rental-audit";
import { rentalAuditMessagesEn } from "@/lib/client-rental-audit-messages";
import { formatWalletUnits } from "@/lib/market-wallet-money";
import { formatUsdMoney } from "@/lib/market-money";
import { DASHBOARD_CLIENT_MARKET_PATH } from "@/lib/dashboard-nav";
import type { MessageKey } from "@/lib/i18n";
import { CLIENT_MARKET_POLL_MS, endReasonKey } from "./host-utils";
import { ClientAfterTrialDialog, type ClientAfterTrialTarget } from "./client-after-trial-dialog";
import { ClientEgressLine, useBilledTime } from "./online-shop";

/** Mount the buyer's account records and pending reads for the signed-in user. */
export function ClientRentalRecords(props: {
  historyOnly?: boolean; currentOnly?: boolean; focusKey?: string | null; revision?: number;
  onChanged?: () => void; onTerminationOpen?: (id: string) => void;
}) {
  const { session } = useAuth();
  const actor = session?.authenticated ? session.user?.id : undefined;
  return actor ? <RentalWorkspace key={actor} {...props} /> : null;
}

function RentalWorkspace({ historyOnly = false, currentOnly = false, focusKey, revision = 0, onChanged, onTerminationOpen }: {
  historyOnly?: boolean; currentOnly?: boolean; focusKey?: string | null; revision?: number;
  onChanged?: () => void; onTerminationOpen?: (id: string) => void;
}) {
  const [localRevision, setLocalRevision] = React.useState(0);
  const [terminationId, setTerminationId] = React.useState<string | null>(null);
  const changed = React.useCallback(() => { setLocalRevision(n => n + 1); onChanged?.(); }, [onChanged]);
  const openTermination = onTerminationOpen ?? setTerminationId;
  return <div className="grid min-w-0 gap-5" data-testid="client-rental-records">
    {!historyOnly ? <RentalSection view="current" focusKey={focusKey} revision={revision + localRevision} onChanged={changed} onTerminationOpen={openTermination} /> : null}
    {!currentOnly ? <RentalSection view="history" focusKey={focusKey} revision={revision + localRevision} onChanged={changed} onTerminationOpen={openTermination} /> : null}
    {!onTerminationOpen ? <ClientTerminationDialog terminationId={terminationId} initialRole="buyer" onClose={() => setTerminationId(null)} onChanged={changed} /> : null}
  </div>;
}

function RentalSection({ view, focusKey, revision, onChanged, onTerminationOpen }: {
  view: ClientRentalView; focusKey?: string | null; revision: number;
  onChanged: () => void; onTerminationOpen: (id: string) => void;
}) {
  const { locale, t } = useLocaleText();
  const [page, setPage] = React.useState<ClientRentalPage | null>(null);
  const [busy, setBusy] = React.useState(true);
  const [failed, setFailed] = React.useState(false);
  const [expanded, setExpanded] = React.useState(false);
  const [jump, setJump] = React.useState("1");
  const [invalidJump, setInvalidJump] = React.useState(false);
  const [focused, setFocused] = React.useState<string | null>(null);
  const request = React.useRef<AbortController | null>(null);
  const canonicalPage = React.useRef(1);
  const followFocus = React.useRef(focusKey);
  const jumpId = React.useId();
  const titleId = React.useId();
  const revokeAccess = React.useCallback(() => {
    request.current?.abort(); request.current = null;
    setPage(null); setFocused(null); setBusy(false); setFailed(true);
  }, []);
  const load = React.useCallback(async (number: number, options?: { silent?: boolean; replace?: boolean; focus?: string | null }) => {
    if (options?.silent && request.current && !options.replace) return;
    request.current?.abort();
    const controller = new AbortController(); request.current = controller;
    if (!options?.silent) { setBusy(true); setFailed(false); }
    try {
      const next = await getClientRentals(view, number, options?.focus, controller.signal);
      if (controller.signal.aborted) return;
      canonicalPage.current = next.page;
      setPage(next); setJump(String(next.page)); setFailed(false);
      if (next.focusedId) setFocused(next.focusedId);
    } catch (cause) {
      if (controller.signal.aborted) return;
      setFailed(true);
      if (cause instanceof ApiError && (cause.status === 401 || cause.status === 403)) {
        setPage(null); setFocused(null);
      }
    } finally {
      if (request.current === controller) { request.current = null; setBusy(false); }
    }
  }, [view]);
  React.useEffect(() => {
    setFocused(null); canonicalPage.current = 1; followFocus.current = focusKey;
    void load(1, { focus: focusKey });
    return () => request.current?.abort();
  }, [load, focusKey]);
  const previousRevision = React.useRef(revision);
  React.useEffect(() => {
    if (previousRevision.current === revision) return;
    previousRevision.current = revision;
    // Preserve expanded details while re-reading a page after a completed action.
    void load(canonicalPage.current, { silent: true, replace: true, focus: followFocus.current });
  }, [load, revision]);
  React.useEffect(() => {
    const poll = () => {
      if ((!expanded || page?.items.some((item) => item.status === "opening")) && document.visibilityState === "visible") void load(canonicalPage.current, { silent: true, focus: followFocus.current });
    };
    const timer = window.setInterval(poll, CLIENT_MARKET_POLL_MS);
    document.addEventListener("visibilitychange", poll);
    return () => { window.clearInterval(timer); document.removeEventListener("visibilitychange", poll); };
  }, [expanded, load, page]);
  const changePage = (number: number) => {
    if (busy || !page || number < 1 || number > page.totalPages) return;
    followFocus.current = null;
    setFocused(null); setInvalidJump(false); setPage(null); void load(number);
  };
  const money = (value: string) => formatWalletUnits(value, "$", locale);
  const date = (value: string | null) => value ? new Intl.DateTimeFormat(locale, { dateStyle: "short", timeStyle: "short" }).format(new Date(value)) : "—";
  const columns: RecordTableColumn<ClientRentalRecord>[] = [
    { id: "client", header: t("online.clientColumn"), cell: row => <div className="grid min-w-0 gap-0.5"><span className="truncate font-medium" title={clientRentalLabel(row)}>{clientRentalLabel(row)}</span><span className="truncate text-xs text-muted-foreground md:hidden">{row.providerEmail || "—"}</span></div> },
    { id: "provider", header: t("account.market.col.provider"), minWidth: "md", cell: row => <span className="block truncate text-muted-foreground" title={row.providerEmail || undefined}>{row.providerEmail || "—"}</span> },
    { id: "status", header: t("account.market.col.status"), className: "w-20 sm:w-40", cell: row => <div className="grid gap-0.5"><span className="truncate">{t(row.status === "free" ? "shareMarket.free" : `rentalAudit.status.${row.status}`)}</span>{row.attention !== "none" ? <span className="truncate text-xs text-amber-800" title={t(`rentalAudit.attention.${row.attention}`)}>{t(`rentalAudit.attention.${row.attention}`)}</span> : null}</div> },
    ...(view === "history" ? [
      { id: "ended", header: t("rentalAudit.ended"), minWidth: "lg" as const, className: "w-40", cell: (row: ClientRentalRecord) => <span className="text-muted-foreground">{date(row.endedAt)}</span> },
      { id: "net", header: t("clientRentals.net"), className: "w-20 sm:w-24", cell: (row: ClientRentalRecord) => <span className="block truncate tabular-nums" title={row.money ? undefined : t("rentalAudit.moneyUnavailable")}>{row.money ? money(row.money.netUnits) : "—"}</span> },
    ] : [
      { id: "price", header: t("account.market.col.price"), className: "w-20 sm:w-32", cell: (row: ClientRentalRecord) => <span className="block truncate tabular-nums">{row.cyclePriceMinor !== null ? t("rentalAudit.monthlyPrice", { price: formatUsdMoney(row.cyclePriceMinor, locale) }) : row.dailyRateMinor !== null ? t("rentalAudit.dailyPrice", { price: formatUsdMoney(row.dailyRateMinor, locale) }) : row.pricingModel === "free" ? t("shareMarket.free") : "—"}</span> },
      { id: "term", header: t("account.market.col.term"), minWidth: "lg" as const, className: "w-40", cell: (row: ClientRentalRecord) => <span className="text-muted-foreground">{date(row.periodEnd || row.expiresAt)}</span> },
    ]),
  ];
  return <section className="grid min-w-0 gap-2" aria-labelledby={titleId} data-testid={`client-rentals-${view}`}>
    <h4 id={titleId} className="text-sm font-semibold text-muted-foreground">{t(view === "current" ? "clientRentals.current" : "clientRentals.history")}{page ? <span className="ml-1.5 tabular-nums">· {page.total}</span> : null}</h4>
    {failed ? <div role="alert" className="flex flex-wrap items-center gap-2 text-sm text-destructive">{t("clientRentals.error")}<Button size="sm" variant="ghost" onPress={() => void load(canonicalPage.current)}>{t("common.retry")}</Button></div> : null}
    {busy ? <p role="status" className="flex items-center gap-2 py-6 text-sm text-muted-foreground"><Loader2 aria-hidden className="h-4 w-4 animate-spin" />{t("common.loading")}</p> : page ? <ExpandableRecordTable
      rows={page.items} rowKey={row => row.id} rowLabel={clientRentalLabel} columns={columns}
      ariaLabel={t(view === "current" ? "clientRentals.current" : "clientRentals.history")} focusKey={focused}
      rowTone={row => row.cleanupStatus === "failed" ? "danger" : row.attention !== "none" ? "attention" : undefined}
      onExpandedChange={setExpanded}
      renderDetail={row => <RentalDetail key={row.id} id={row.id} refreshKey={JSON.stringify([row.status, row.attention, row.cleanupStatus, row.money, row.periodEnd, row.renewalPolicy])} onChanged={onChanged} onTerminationOpen={onTerminationOpen} onForbidden={revokeAccess} />}
      empty={<p className="rounded-lg border border-dashed border-border px-4 py-8 text-center text-sm text-muted-foreground">{t(view === "current" ? "clientRentals.currentEmpty" : "clientRentals.historyEmpty")}{view === "current" ? <Link href={DASHBOARD_CLIENT_MARKET_PATH} className="ml-2 text-accent hover:underline">{t("account.client.openMarket")}</Link> : null}</p>}
    /> : null}
    {page && page.total > 0 ? <nav className="flex flex-wrap items-center justify-between gap-2 text-xs text-muted-foreground" aria-label={t("clientRentals.pagination")}>
      <span className="tabular-nums">{t("clientRentals.count", { count: page.total, page: page.page, pages: page.totalPages })}</span>
      <div className="flex flex-wrap items-center gap-1"><Button size="sm" variant="outline" isDisabled={busy || page.page <= 1} onPress={() => changePage(page.page - 1)}><ChevronLeft aria-hidden className="h-3.5 w-3.5" />{t("common.previous")}</Button><Button size="sm" variant="outline" isDisabled={busy || page.page >= page.totalPages} onPress={() => changePage(page.page + 1)}>{t("common.next")}<ChevronRight aria-hidden className="h-3.5 w-3.5" /></Button>
      <form className="flex items-center gap-1" onSubmit={event => { event.preventDefault(); const next = parseAuditPageNumber(jump, page.totalPages); setInvalidJump(next === null); if (next !== null) changePage(next); }}><label htmlFor={jumpId} className="sr-only">{t("rentalAudit.pageNumber")}</label><input id={jumpId} inputMode="numeric" maxLength={10} value={jump} disabled={busy} aria-invalid={invalidJump} aria-describedby={invalidJump ? `${jumpId}-error` : undefined} className="h-9 w-14 min-w-0 rounded-md border border-border bg-card px-2 text-foreground outline-none focus-visible:ring-2 focus-visible:ring-ring" onChange={e => { setJump(e.target.value); setInvalidJump(false); }} /><Button size="sm" variant="outline" type="submit" isDisabled={busy}>{t("rentalAudit.jump")}</Button></form></div>
      {invalidJump ? <p id={`${jumpId}-error`} role="alert" className="w-full text-destructive">{t("rentalAudit.invalidPage", { pages: page.totalPages })}</p> : null}
    </nav> : null}
  </section>;
}

function RentalDetail({ id, refreshKey, onChanged, onTerminationOpen, onForbidden }: {
  id: string; refreshKey: string; onChanged: () => void; onTerminationOpen: (id: string) => void; onForbidden: () => void;
}) {
  const { locale, t } = useLocaleText();
  const { openChat } = useClientChat();
  const [detail, setDetail] = React.useState<ClientRentalDetail | null>(null);
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState(false);
  const request = React.useRef<AbortController | null>(null);
  const latest = React.useRef(detail); latest.current = detail;
  const load = React.useCallback(async (kind: "initial" | "refresh" | "periods" | "events" = "initial") => {
    request.current?.abort(); const controller = new AbortController(); request.current = controller;
    setBusy(true); setError(false);
    const previous = latest.current;
    try {
      const next = await getClientRentalDetail(id, kind === "periods" ? { periodCursor: previous?.periodsNextCursor ?? undefined } : kind === "events" ? { eventCursor: previous?.eventsNextCursor ?? undefined } : {}, controller.signal);
      if (controller.signal.aborted) return;
      setDetail(current => {
        if (!current || kind === "initial") return next;
        if (kind === "periods") return { ...next, periods: [...current.periods, ...next.periods.filter(p => !current.periods.some(old => old.id === p.id))], events: current.events, eventsNextCursor: current.eventsNextCursor };
        if (kind === "events") return { ...next, events: [...current.events, ...next.events.filter(e => !current.events.some(old => old.id === e.id))], periods: current.periods, periodsNextCursor: current.periodsNextCursor };
        return { ...next,
          periods: [...next.periods, ...current.periods.filter(p => !next.periods.some(head => head.id === p.id))],
          periodsNextCursor: current.periods.length > next.periods.length ? current.periodsNextCursor : next.periodsNextCursor,
          events: [...next.events, ...current.events.filter(e => !next.events.some(head => head.id === e.id))],
          eventsNextCursor: current.events.length > next.events.length ? current.eventsNextCursor : next.eventsNextCursor,
        };
      });
    } catch (cause) {
      if (controller.signal.aborted) return;
      setError(true);
      if (cause instanceof ApiError && (cause.status === 401 || cause.status === 403 || cause.status === 404)) { setDetail(null); onForbidden(); }
    } finally { if (request.current === controller) { request.current = null; setBusy(false); } }
  }, [id, onForbidden]);
  React.useEffect(() => { void load("refresh"); return () => request.current?.abort(); }, [load, refreshKey]);
  const opening = detail?.record.status === "opening";
  const migrationRequired = detail?.rental?.migrationRequired === true;
  React.useEffect(() => {
    if (!opening && !migrationRequired) return;
    const poll = () => { if (!request.current && document.visibilityState === "visible") void load("refresh"); };
    const timer = window.setInterval(poll, opening ? 5000 : CLIENT_MARKET_POLL_MS);
    document.addEventListener("visibilitychange", poll);
    return () => { clearInterval(timer); document.removeEventListener("visibilitychange", poll); };
  }, [opening, migrationRequired, load]);
  const changed = React.useCallback(() => { void load("refresh"); onChanged(); }, [load, onChanged]);
  const row = detail?.record;
  const rental = detail?.rental;
  const [afterTrial, setAfterTrial] = React.useState<ClientAfterTrialTarget | null>(null);
  const billedTime = useBilledTime();
  const money = (value: string) => formatWalletUnits(value, "$", locale);
  const date = (value: string | null) => value ? new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short" }).format(new Date(value)) : "—";
  const price = (record: ClientRentalRecord) =>
    record.cyclePriceMinor !== null
      ? t("rentalAudit.monthlyPrice", { price: formatUsdMoney(record.cyclePriceMinor, locale) })
      : record.dailyRateMinor !== null
        ? t("rentalAudit.dailyPrice", { price: formatUsdMoney(record.dailyRateMinor, locale) })
        : record.pricingModel === "free"
          ? t("shareMarket.free")
          : "—";
  // The trial-only choice can still change while the trial runs and nothing was paid.
  const trialChoice = !!row && !!rental && !!row.contractId && row.group === "current" && !rental.reclaimEffectiveAt
    && (row.cyclePriceMinor ?? 0) > 0 && (row.trialSecondsRemaining ?? 0) > 0
    && ["pending_activation", "trialing"].includes(rental.contractStatus || row.contractStatus || "") && !!rental.afterTrial;
  return (
    <div className="grid min-w-0 gap-3" data-testid="client-rental-detail">
      {busy ? (
        <p role="status" className="text-sm text-muted-foreground">
          {t("common.loading")}
        </p>
      ) : null}
      {error ? (
        <div role="alert" className="text-sm text-destructive">
          {t("clientRentals.detailError")}
          <Button variant="ghost" size="sm" onPress={() => void load()}>
            {t("common.retry")}
          </Button>
        </div>
      ) : null}
      {detail && row ? (
        <>
          {row.status === "activation_failed" && <p role="status">{t("lease.activationFailed")}</p>}
          {rental?.reclaimEffectiveAt ? (
            <div role="status" className="flex gap-2.5 rounded-xl border border-amber-200 bg-amber-50 px-3 py-2.5 text-sm leading-6 text-amber-950">
              <TriangleAlert aria-hidden className="mt-1 size-4 shrink-0 text-amber-600" />
              <p className="min-w-0">
                {t("clientMarket.mine.reclaimBanner", { date: date(new Date(rental.reclaimEffectiveAt * 1000).toISOString()) })}
                {rental.reclaimReason ? ` ${t("clientMarket.mine.reclaimReason", { reason: t(`clientMarket.reclaim.reason.${rental.reclaimReason}`) })}` : ""}
                {rental.reclaimNote ? <span className="mt-1 block whitespace-pre-wrap break-words text-amber-900">{t("clientMarket.mine.reclaimNote", { note: rental.reclaimNote })}</span> : null}
              </p>
            </div>
          ) : null}
          {rental && <div className="grid gap-3 rounded-lg border border-border p-3 text-sm">
            {rental.migrationRequired && <p>{t("lease.migrationHelp")}</p>}
            {opening && <p role="status">{t("lease.provisioning", { time: date(new Date(rental.activationDeadline
              ? rental.activationDeadline * 1000 : Date.parse(row.createdAt) + 600000).toISOString()) })}</p>}
            {rental.egressIp || rental.country ? (
              <div className="grid gap-1">
                <span className="text-xs text-muted-foreground">{t("clientRentals.egress")}</span>
                <ClientEgressLine listing={{ country: rental.country ?? row.countryCode, egressIp: rental.egressIp ?? null }} emphasize />
                {rental.egressIp ? <span className="text-xs text-muted-foreground">{t("clientMarket.mine.ipHint")}</span> : null}
                {rental.egressDiffers ? <span className="text-xs text-amber-800">{t("clientRentals.egressDiffers")}</span> : null}
              </div>
            ) : null}
            <RecordDetailList
              items={[
                { label: t("rentalAudit.status"), value: t(row.status === "free" ? "shareMarket.free" : `rentalAudit.status.${row.status}`) },
                { label: t("rentalAudit.price"), value: price(row) },
                { label: t("account.market.col.term"), value: date(row.periodEnd || row.expiresAt) },
                ...(billedTime(rental) ? [{ label: t("clientRentals.billedHours"), value: billedTime(rental) }] : []),
                ...(trialChoice ? [{
                  label: t("clientMarket.mine.afterTrial"),
                  value: <button type="button" className="rounded text-left font-medium text-accent hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent" onClick={() => setAfterTrial({
                    contractId: row.contractId!, name: rental.title || clientRentalLabel(row), to: rental.afterTrial === "end" ? "continue" : "end",
                    price: row.cyclePriceMinor != null ? formatUsdMoney(row.cyclePriceMinor, locale) : "", trialSecondsRemaining: row.trialSecondsRemaining ?? 0,
                  })}>
                    {t(rental.afterTrial === "end" ? "clientMarket.mine.afterTrialEnd" : "clientMarket.mine.afterTrialContinue")}
                    <span className="ml-1 text-xs font-normal text-muted-foreground">· {t("clientMarket.mine.change")}</span>
                  </button>,
                }] : []),
              ]}
            />
            {rental.clientUrl && row.activatedAt && <>
              <p className="break-all"><span className="text-muted-foreground">{t("clientMarket.shop.address")} · </span>{rental.clientUrl}</p>
              <p className="break-all text-xs text-muted-foreground">{t("lease.usage", { host: "<slug>--" + new URL(rental.clientUrl).host })}</p>
            </>}
            {rental.grantUntil && <p className="text-xs">{t("clientMarket.shop.grant")} · {date(new Date(rental.grantUntil * 1000).toISOString())}</p>}
            {rental.holdStatus && <p className="text-xs text-muted-foreground">{t(`lease.hold.${rental.holdStatus}`)}</p>}
            <LeaseActions id={rental.leaseId} upgradeTarget={rental.canUpgrade ? clientRentalLabel(row) : undefined} active={!!row.activatedAt && !row.endedAt} address={rental.clientUrl} canEnd={!row.endedAt} onChanged={changed} />
          </div>}
          {!rental ? (
            <RecordDetailList
              items={[
                { label: t("rentalAudit.status"), value: t(row.status === "free" ? "shareMarket.free" : `rentalAudit.status.${row.status}`) },
                { label: t("account.market.col.provider"), value: row.providerEmail || "—" },
                { label: t("rentalAudit.price"), value: price(row) },
                { label: t("rentalAudit.ended"), value: date(row.endedAt) },
              ]}
            />
          ) : null}
          {row.group === "history" && row.attention !== "none" ? (
            <p className="text-xs text-amber-800">{t("clientRentals.followup")}</p>
          ) : null}
          {detail.settlement ? (
            <section className="grid gap-2 rounded-lg border border-border p-3">
              <h5 className="text-sm font-medium">{t("clientRentals.settlement")}</h5>
              <RecordDetailList
                items={[
                  { label: t("clientRentals.reason"), value: t(endReasonKey(detail.settlement.reason)) },
                  {
                    label: t("clientRentals.responsibility"),
                    value: t(
                      detail.settlement.responsibility === "buyer"
                        ? "clientRentals.responsibility.buyer"
                        : detail.settlement.responsibility === "undetermined"
                          ? "clientRentals.responsibility.undetermined"
                          : "clientRentals.responsibility.provider",
                    ),
                  },
                  { label: t("rentalAudit.held"), value: money(detail.settlement.heldUnits) },
                  { label: t("rentalAudit.returnedUnits"), value: money(detail.settlement.returnedUnits) },
                  {
                    label: t("rentalAudit.compensationUnits"),
                    value: money(detail.settlement.compensationUnits),
                  },
                ]}
              />
              {row.terminationId ? (
                <Button
                  variant="outline"
                  size="sm"
                  className="justify-self-start"
                  onPress={() => onTerminationOpen(row.terminationId!)}
                >
                  {t("clientRentals.discussion")}
                </Button>
              ) : null}
            </section>
          ) : null}
          {rental && row.group === "current" ? (
            <>
              {row.cyclePriceMinor != null && row.contractId ? (
                <MarketWalletRenewalCard
                  contractId={row.contractId}
                  refreshKey={JSON.stringify([
                    row.contractStatus,
                    row.renewalPolicy,
                    row.periodEnd,
                    row.trialSecondsRemaining,
                  ])}
                  compact
                  onChanged={changed}
                />
              ) : null}
              {rental.canDispute && rental.contractId ? (
                <MarketServiceDisputeControl
                  contractId={rental.contractId}
                  initiallyOpen={rental.openDispute}
                  onChanged={changed}
                />
              ) : null}
            </>
          ) : null}
          <details className="group min-w-0 rounded-lg border border-border">
            <summary className="flex min-h-11 cursor-pointer list-none items-center justify-between gap-3 px-3 text-sm font-medium focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent [&::-webkit-details-marker]:hidden">
              {t("clientRentals.ledger")}
              <ChevronDown aria-hidden className="size-4 shrink-0 text-muted-foreground transition-transform group-open:rotate-180 motion-reduce:transition-none" />
            </summary>
            <div className="grid gap-3 border-t border-border p-3">
              <RecordDetailList
                items={[
                  { label: t("clientRentals.recordId"), value: <span className="break-all">{row.id}</span> },
                  { label: t("online.clientColumn"), value: <span className="break-all">{clientRentalLabel(row)}</span> },
                  { label: t("account.market.col.provider"), value: row.providerEmail || "—" },
                  { label: t("rentalAudit.created"), value: date(row.createdAt) },
                  { label: t("rentalAudit.activated"), value: date(row.activatedAt) },
                  { label: t("rentalAudit.ended"), value: date(row.endedAt) },
                  { label: t("rentalAudit.cleanupAt"), value: date(row.cleanupConfirmedAt) },
                  { label: t("rentalAudit.attention"), value: t(`rentalAudit.attention.${row.attention}`) },
                  { label: t("clientRentals.cleanup"), value: t(`rentalAudit.cleanup.${row.cleanupStatus}`) },
                ]}
              />
              {row.trialSecondsRemaining !== null ? (
                <p className="text-xs text-muted-foreground">
                  {t("clientRentals.trial", {
                    used: detail.trialConsumedSeconds,
                    remaining: row.trialSecondsRemaining,
                  })}
                </p>
              ) : null}
              <section className="grid gap-2">
                <p className="text-xs text-muted-foreground">{t("clientRentals.financialHelp")}</p>
                {row.money ? (
                  <RecordDetailList
                    items={CLIENT_RENTAL_MONEY_FIELDS.map((key) => ({
                      label: t(key === "netUnits" ? "clientRentals.netUnits" : `rentalAudit.${key}`),
                      value: money(row.money![key]),
                    }))}
                  />
                ) : (
                  <p className="text-xs text-muted-foreground">{t("rentalAudit.moneyUnavailable")}</p>
                )}
              </section>
              <div className="grid gap-2 text-xs">
                <strong className="text-sm font-medium">{t("rentalAudit.periods")}</strong>
                {detail.periods.length ? (
                  detail.periods.map((p) => (
                    <div key={p.id} className="grid gap-1 border-t border-border pt-2">
                      <strong>{t("rentalAudit.period", { sequence: p.sequence })}</strong>
                      <span>
                        {date(p.startedAt)} → {date(p.endedAt)}
                      </span>
                      <span>
                        {t("clientRentals.periodAmount", {
                          amount: money(p.amountUnits),
                          returned: money(p.returnedUnits),
                        })}
                      </span>
                    </div>
                  ))
                ) : (
                  <p className="text-muted-foreground">{t("rentalAudit.noPeriods")}</p>
                )}
                {detail.periodsNextCursor !== null ? (
                  <Button size="sm" variant="ghost" isDisabled={busy} onPress={() => void load("periods")}>
                    {t("rentalAudit.morePeriods")}
                  </Button>
                ) : null}
              </div>
              <div className="grid gap-2 text-xs">
                <strong className="text-sm font-medium">{t("rentalAudit.events")}</strong>
                <ol className="grid gap-2">
                  {detail.events.map((event) => {
                    const key = `rentalAudit.event.${event.action}`;
                    return (
                      <li key={event.id} className="grid min-w-0 gap-1 border-t border-border pt-2">
                        <span>
                          {date(event.createdAt)} ·{" "}
                          {t((key in rentalAuditMessagesEn ? key : "rentalAudit.event.updated") as MessageKey)}
                          {(event.occurrences ?? 0) > 1 && ` ×${event.occurrences}`}
                        </span>
                        {event.note ? (
                          <p className="whitespace-pre-wrap break-words text-muted-foreground">{event.note}</p>
                        ) : null}
                      </li>
                    );
                  })}
                </ol>
                {detail.eventsNextCursor ? (
                  <Button size="sm" variant="ghost" isDisabled={busy} onPress={() => void load("events")}>
                    {t("rentalAudit.moreEvents")}
                  </Button>
                ) : null}
              </div>
            </div>
          </details>
          <div className="flex flex-wrap items-center justify-between gap-2">
            <p className="text-xs text-muted-foreground">
              {t(row.provenance === "historical_record" ? "rentalAudit.historical" : "rentalAudit.snapshot")}
            </p>
            {row.installationId ? (
              <Button size="sm" variant="ghost" onPress={() => void openChat(row.installationId!)}>
                <MessageCircle aria-hidden className="h-4 w-4" />
                {t("clientTermination.chat")}
              </Button>
            ) : null}
          </div>
          <ClientAfterTrialDialog target={afterTrial} onClose={() => setAfterTrial(null)} onChanged={changed} />
        </>
      ) : null}
    </div>
  );
}
