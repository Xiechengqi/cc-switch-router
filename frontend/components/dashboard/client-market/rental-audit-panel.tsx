"use client";

import * as React from "react";
import { Button } from "@heroui/react";
import { ChevronDown, ChevronLeft, ChevronRight, Loader2, RefreshCw, Search } from "lucide-react";
import { useAuth } from "@/components/auth/auth-provider";
import { CompactSelect } from "@/components/common/compact-select";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { ApiError } from "@/lib/api";
import { AUDIT_ATTENTION, AUDIT_MONEY_FIELDS, AUDIT_STATUSES, auditFiltersFromDates, getRentalAudit, getRentalAuditDetail, type AuditDetail, type AuditFilters, type AuditPage, type AuditRecord } from "@/lib/client-rental-audit";
import { formatWalletUnits } from "@/lib/market-wallet-money";
import { rentalAuditMessagesEn } from "@/lib/client-rental-audit-messages";
import type { MessageKey } from "@/lib/i18n";

const FIELD = "h-9 w-full min-w-0 rounded-lg border border-border bg-white px-2 text-sm text-foreground outline-none focus-visible:ring-2 focus-visible:ring-accent";
const INITIAL = { search: "", buyerEmail: "", providerEmail: "", status: "", attention: "", startDate: "", endDate: "" };
function Field({ label, children, as: Tag = "label" }: { label: string; children: React.ReactNode; as?: "label" | "div" }) {
  return <Tag className="grid min-w-0 gap-1 text-xs text-muted-foreground"><span>{label}</span>{children}</Tag>;
}

function useAuditColumnCount() {
  const [count, setCount] = React.useState(6);
  React.useEffect(() => {
    const breakpoints = [640, 768, 1024].map(width => window.matchMedia(`(min-width: ${width}px)`));
    const update = () => setCount(3 + breakpoints.filter(query => query.matches).length);
    update();
    for (const query of breakpoints) query.addEventListener("change", update);
    return () => { for (const query of breakpoints) query.removeEventListener("change", update); };
  }, []);
  return count;
}

/** Separate from public Host filtering/polling. Unmount on capability/account
 * changes so no previous owner's emails or money remain in another session. */
export function ClientRentalAuditPanel({ onTerminationOpen, revision = 0 }: { onTerminationOpen: (id: string) => void; revision?: number }) {
  const { session } = useAuth();
  const actor = session?.authenticated && session.canAuditClientMarket === true ? session.user?.id : undefined;
  return actor ? <AuditWorkspace key={actor} onTerminationOpen={onTerminationOpen} revision={revision} /> : null;
}

function AuditWorkspace({ onTerminationOpen, revision }: { onTerminationOpen: (id: string) => void; revision: number }) {
  const { t } = useLocaleText();
  const [draft, setDraft] = React.useState(INITIAL);
  const [filters, setFilters] = React.useState<AuditFilters>({});
  const [cursors, setCursors] = React.useState<(string | undefined)[]>([undefined]);
  const [refresh, setRefresh] = React.useState(0);
  const [loaded, setLoaded] = React.useState<{ scope: string; page: AuditPage } | null>(null);
  const [loading, setLoading] = React.useState(true);
  const [error, setError] = React.useState<"error" | "invalidDates" | null>(null);
  const [selected, setSelected] = React.useState<string | null>(null);
  const [filtersOpen, setFiltersOpen] = React.useState(false);
  const filterId = React.useId();
  // A colspan that includes display:none columns creates phantom table tracks
  // on small screens and squeezes the visible Client column when expanded.
  const columnCount = useAuditColumnCount();
  const cursor = cursors[cursors.length - 1];
  const scope = JSON.stringify([filters, cursor]);
  const page = loaded?.scope === scope ? loaded.page : null;
  React.useEffect(() => { setSelected(null); }, [scope]);
  React.useEffect(() => {
    const controller = new AbortController();
    setLoading(true); setError(null);
    void getRentalAudit(filters, cursor, controller.signal).then(value => {
      if (!controller.signal.aborted) setLoaded({ scope, page: value });
    }).catch(error => {
      if (controller.signal.aborted) return;
      if (error instanceof ApiError && (error.status === 401 || error.status === 403)) { setLoaded(null); setSelected(null); }
      setError("error");
    })
      .finally(() => { if (!controller.signal.aborted) setLoading(false); });
    return () => controller.abort();
  }, [filters, cursor, refresh, revision, scope]);
  const apply = (event: React.FormEvent) => {
    event.preventDefault();
    try { setFilters(auditFiltersFromDates(draft)); setCursors([undefined]); setRefresh(n => n + 1); }
    catch { setError("invalidDates"); }
  };
  return <section aria-labelledby="client-rental-audit-title" className="grid min-w-0 gap-3 rounded-xl border border-border bg-card p-3 sm:p-4" data-testid="client-rental-audit">
    <div className="flex flex-wrap items-center justify-between gap-2">
      <div><h2 id="client-rental-audit-title" className="text-base font-semibold">{t("rentalAudit.title")}</h2><p className="text-xs text-muted-foreground">{t("rentalAudit.ownerOnly")}</p></div>
      <Button size="sm" variant="outline" isDisabled={loading} onClick={() => setRefresh(n => n + 1)}><RefreshCw className="h-3.5 w-3.5" />{t("rentalAudit.refresh")}</Button>
    </div>
    <form onSubmit={apply} className="grid gap-3">
      <div className="flex flex-wrap items-end gap-2">
        <div className="min-w-0 flex-[1_1_16rem]"><Field label={t("rentalAudit.search")}><input className={FIELD} maxLength={254} value={draft.search} placeholder={t("rentalAudit.searchHint")} onChange={e => setDraft(v => ({ ...v, search: e.target.value }))} /></Field></div>
        <div className="min-w-0 flex-[1_1_10rem] sm:max-w-48"><Field as="div" label={t("rentalAudit.status")}><CompactSelect ariaLabel={t("rentalAudit.status")} className="w-full" value={draft.status} onChange={status => setDraft(v => ({ ...v, status }))}
          options={[{ value: "", label: t("rentalAudit.all") }, ...AUDIT_STATUSES.map(value => ({ value, label: t(`rentalAudit.status.${value}`) }))]} /></Field></div>
        <Button size="sm" variant="primary" type="submit"><Search className="h-3.5 w-3.5" />{t("rentalAudit.apply")}</Button>
        <Button size="sm" variant="ghost" type="button" aria-expanded={filtersOpen} aria-controls={filterId} onPress={() => setFiltersOpen(open => !open)}>{t("rentalAudit.filters")}<ChevronDown aria-hidden className={`h-3.5 w-3.5 ${filtersOpen ? "rotate-180" : ""}`} /></Button>
        {Object.values(draft).some(Boolean) || Object.keys(filters).length ? <Button size="sm" variant="ghost" type="button" onClick={() => { setDraft(INITIAL); setFilters({}); setCursors([undefined]); setRefresh(n => n + 1); }}>{t("common.reset")}</Button> : null}
      </div>
      <div id={filterId} hidden={!filtersOpen}>
        <div className="grid gap-2 sm:grid-cols-2 lg:grid-cols-3">
          <Field label={t("rentalAudit.buyer")}><input className={FIELD} maxLength={254} value={draft.buyerEmail} onChange={e => setDraft(v => ({ ...v, buyerEmail: e.target.value }))} /></Field>
          <Field label={t("rentalAudit.provider")}><input className={FIELD} maxLength={254} value={draft.providerEmail} onChange={e => setDraft(v => ({ ...v, providerEmail: e.target.value }))} /></Field>
          <Field as="div" label={t("rentalAudit.attention")}><CompactSelect ariaLabel={t("rentalAudit.attention")} className="w-full" value={draft.attention} onChange={attention => setDraft(v => ({ ...v, attention }))}
            options={[{ value: "", label: t("rentalAudit.all") }, { value: "any", label: t("rentalAudit.anyAttention") }, ...AUDIT_ATTENTION.map(value => ({ value, label: t(`rentalAudit.attention.${value}`) }))]} /></Field>
          <Field label={t("rentalAudit.from")}><input type="date" className={FIELD} value={draft.startDate} onChange={e => setDraft(v => ({ ...v, startDate: e.target.value }))} /></Field>
          <Field label={t("rentalAudit.until")}><input type="date" className={FIELD} value={draft.endDate} onChange={e => setDraft(v => ({ ...v, endDate: e.target.value }))} /></Field>
        </div>
      </div>
    </form>
    {error ? <p role="alert" className="text-sm text-destructive">{t(`rentalAudit.${error}`)}</p> : null}
    {loading ? <p role="status" className="flex items-center gap-2 text-xs text-muted-foreground"><Loader2 className="h-3.5 w-3.5 animate-spin" />{t("rentalAudit.loading")}</p> : null}
    {page ? <>
      <div className="max-h-[32rem] min-w-0 overflow-auto rounded-lg border border-border" tabIndex={0} role="region" aria-label={t("rentalAudit.title")}>
        <table className="w-full table-fixed text-left text-xs">
          <thead className="sticky top-0 z-10 bg-muted"><tr>
            <th scope="col" className="px-2 py-2 font-medium sm:px-3">Client</th>
            <th scope="col" className="hidden px-3 py-2 font-medium md:table-cell">{t("rentalAudit.buyer")}</th>
            <th scope="col" className="w-24 px-2 py-2 font-medium sm:w-36 sm:px-3">{t("rentalAudit.status")}</th>
            <th scope="col" className="hidden w-28 px-3 py-2 text-right font-medium sm:table-cell">{t("rentalAudit.chargedShort")}</th>
            <th scope="col" className="hidden w-44 px-3 py-2 font-medium lg:table-cell">{t("rentalAudit.orderedShort")}</th>
            <th scope="col" className="w-10"><span className="sr-only">{t("rentalAudit.details")}</span></th>
          </tr></thead>
          <tbody>{page.items.map(record => <AuditRow key={record.id} record={record} expanded={selected === record.id}
            onToggle={() => setSelected(current => current === record.id ? null : record.id)}
            onTerminationOpen={onTerminationOpen} columnCount={columnCount} revision={`${refresh}:${revision}`} />)}</tbody>
        </table>
        {!page.items.length ? <p className="p-5 text-sm text-muted-foreground">{t("rentalAudit.empty")}</p> : null}
      </div>
      <div className="flex flex-wrap items-center justify-between gap-2 text-xs text-muted-foreground"><p>{t("rentalAudit.count", { count: page.total, page: cursors.length })}</p><div className="flex gap-1">
        <Button size="sm" variant="outline" isDisabled={loading || cursors.length === 1} onClick={() => setCursors(v => v.slice(0, -1))}><ChevronLeft className="h-3.5 w-3.5" />{t("common.previous")}</Button>
        <Button size="sm" variant="outline" isDisabled={loading || !page.nextCursor} onClick={() => { if (page.nextCursor) setCursors(v => [...v, page.nextCursor!]); }}>{t("common.next")}<ChevronRight className="h-3.5 w-3.5" /></Button>
      </div></div>
    </> : null}
  </section>;
}

function AuditRow({ record, expanded, onToggle, onTerminationOpen, columnCount, revision }: {
  record: AuditRecord; expanded: boolean; onToggle: () => void; onTerminationOpen: (id: string) => void; columnCount: number; revision: string;
}) {
  const { locale, t } = useLocaleText();
  const detailId = React.useId();
  const client = record.clientSubdomain || record.installationId || record.jobId || record.id;
  const buyer = record.buyerEmail || t("rentalAudit.notRecorded");
  return <>
    <tr data-testid="rental-audit-row" className={`cursor-pointer border-t border-border align-middle hover:bg-muted/50 ${expanded ? "bg-muted/50" : ""}`}
      onClick={event => {
        if ((event.target as HTMLElement).closest("button, a") || window.getSelection()?.toString()) return;
        onToggle();
      }}>
      <td className="px-2 py-3 sm:px-3"><p className="break-all font-medium">{client}</p><p className="mt-1 break-all text-muted-foreground md:hidden">{buyer}</p></td>
      <td className="hidden break-all px-3 py-3 md:table-cell">{buyer}</td>
      <td className="px-2 py-3 sm:px-3"><p>{t(`rentalAudit.status.${record.status}`)}</p>{record.attention !== "none" ? <p className="mt-1 text-amber-700">{t(`rentalAudit.attention.${record.attention}`)}</p> : null}</td>
      <td className="hidden break-all px-3 py-3 text-right tabular-nums sm:table-cell">{record.money ? formatWalletUnits(record.money.chargedUnits, "$", locale) : <span title={t("rentalAudit.moneyUnavailable")}>—</span>}</td>
      <td className="hidden px-3 py-3 text-muted-foreground lg:table-cell"><time dateTime={record.createdAt}>{new Intl.DateTimeFormat(locale, { dateStyle: "short", timeStyle: "short" }).format(new Date(record.createdAt))}</time></td>
      <td className="px-1 py-2"><Button size="sm" variant="ghost" isIconOnly aria-label={t(expanded ? "rentalAudit.collapse" : "rentalAudit.expand", { client })}
        aria-expanded={expanded} aria-controls={detailId} onPress={onToggle}>
        <ChevronDown aria-hidden className={`h-4 w-4 ${expanded ? "rotate-180" : ""}`} />
      </Button></td>
    </tr>
    {expanded ? <tr><td colSpan={columnCount} className="border-t border-border bg-muted/20 p-3 sm:p-4"><RentalDetail id={record.id} regionId={detailId} revision={revision} onTerminationOpen={onTerminationOpen} /></td></tr> : null}
  </>;
}

function RentalDetail({ id, regionId, revision, onTerminationOpen }: { id: string; regionId: string; revision: string; onTerminationOpen: (id: string) => void }) {
  const { locale, t } = useLocaleText();
  const [detail, setDetail] = React.useState<AuditDetail | null>(null);
  const [busy, setBusy] = React.useState(true);
  const [error, setError] = React.useState(false);
  const request = React.useRef<AbortController | null>(null);
  const latest = React.useRef(detail); latest.current = detail;
  const load = React.useCallback(async (kind: "initial" | "periods" | "events" = "initial") => {
    request.current?.abort(); const controller = new AbortController(); request.current = controller;
    setBusy(true); setError(false);
    const previous = latest.current;
    try {
      const next = await getRentalAuditDetail(id, kind === "periods" ? { periodCursor: previous?.periodsNextCursor ?? undefined } : kind === "events" ? { eventCursor: previous?.eventsNextCursor ?? undefined } : {}, controller.signal);
      if (controller.signal.aborted) return;
      setDetail(current => {
        if (!current || kind === "initial") return next;
        if (kind === "periods") return { ...next, periods: [...current.periods, ...next.periods.filter(p => !current.periods.some(old => old.id === p.id))], events: current.events, eventsNextCursor: current.eventsNextCursor };
        return { ...next, events: [...current.events, ...next.events.filter(e => !current.events.some(old => old.id === e.id))], periods: current.periods, periodsNextCursor: current.periodsNextCursor };
      });
    } catch (error) {
      if (!controller.signal.aborted) {
        if (error instanceof ApiError && (error.status === 401 || error.status === 403)) setDetail(null);
        setError(true);
      }
    }
    finally { if (!controller.signal.aborted) setBusy(false); }
  }, [id]);
  React.useEffect(() => { void load(); return () => request.current?.abort(); }, [load, revision]);
  const time = (v: string | null) => v ? new Date(v).toLocaleString(locale) : "—";
  const record = detail?.record;
  const price = record?.cyclePriceMinor !== null && record?.cyclePriceMinor !== undefined
    ? t("rentalAudit.monthlyPrice", { price: formatWalletUnits((BigInt(record.cyclePriceMinor) * 86_400n).toString(), "$", locale) })
    : record?.dailyRateMinor !== null && record?.dailyRateMinor !== undefined
      ? t("rentalAudit.dailyPrice", { price: formatWalletUnits((BigInt(record.dailyRateMinor) * 86_400n).toString(), "$", locale) })
      : record?.pricingModel === "free" ? t("rentalAudit.freePrice") : null;
  const renewal = record?.renewalPolicy === "automatic" || record?.renewalPolicy === "manual"
    ? t(`rentalAudit.renewal.${record.renewalPolicy}`) : null;
  const info = record ? [
    [t("rentalAudit.orderId"), record.id], [t("rentalAudit.jobId"), record.jobId], [t("rentalAudit.installationId"), record.installationId],
    [t("rentalAudit.contractId"), record.contractId], [t("rentalAudit.client"), record.clientSubdomain], ["Host", record.hostId],
    [t("rentalAudit.buyer"), record.buyerEmail], [t("rentalAudit.provider"), record.providerEmail], ["IP", record.hostIp ? `${record.hostIp}${record.hostPort ? `:${record.hostPort}` : ""}` : null],
    [t("rentalAudit.created"), time(record.createdAt)], [t("rentalAudit.activated"), time(record.activatedAt)], [t("rentalAudit.ended"), time(record.endedAt)],
    [t("rentalAudit.cleanupAt"), time(record.cleanupConfirmedAt)], [t("rentalAudit.offer"), record.offerRevision?.toString()],
    [t("rentalAudit.price"), price], [t("rentalAudit.renewal"), renewal],
    [t("rentalAudit.trial"), record.trialSecondsRemaining === null ? "—" : `${detail?.trialConsumedSeconds ?? 0} / ${record.trialSecondsRemaining}`],
  ] : [];
  return <section id={regionId} aria-label={t("rentalAudit.details")} className="grid min-w-0 gap-4" data-testid="rental-audit-detail">
      {busy ? <p role="status" className="text-xs text-muted-foreground">{t("rentalAudit.loading")}</p> : null}
      {error ? <div role="alert" className="text-sm text-destructive">{t("rentalAudit.error")} <Button size="sm" variant="ghost" onClick={() => void load()}>{t("common.retry")}</Button></div> : null}
      {record && detail ? <>
        <div className="flex flex-wrap gap-2 text-xs"><span className="rounded bg-muted px-2 py-1">{t(`rentalAudit.status.${record.status}`)}</span>{record.cleanupStatus !== "not_required" ? <span className="rounded bg-muted px-2 py-1">{t(`rentalAudit.cleanup.${record.cleanupStatus}`)}</span> : null}{record.attention !== "none" && t(`rentalAudit.attention.${record.attention}`) !== t(`rentalAudit.cleanup.${record.cleanupStatus}`) ? <span className="rounded bg-muted px-2 py-1">{t(`rentalAudit.attention.${record.attention}`)}</span> : null}</div>
        <dl className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">{info.map(([label, value]) => <div key={label}><dt className="text-xs text-muted-foreground">{label}</dt><dd className="mt-0.5 break-all text-sm">{value || t("rentalAudit.notRecorded")}</dd></div>)}</dl>
        <p className="text-xs text-muted-foreground">{t(record.provenance === "order_snapshot" ? "rentalAudit.snapshot" : "rentalAudit.historical")}{!record.hostPresent && record.hostId ? ` · ${t("rentalAudit.removed")}` : ""}</p>
        <section className="grid gap-3 rounded-lg border border-border bg-muted/20 p-3"><h3 className="text-sm font-semibold">{t("rentalAudit.money")}</h3><p className="text-xs leading-relaxed text-muted-foreground">{t("rentalAudit.financialHelp")}</p>
          {record.money ? <dl className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">{AUDIT_MONEY_FIELDS.map(field => <div key={field}><dt className="text-xs text-muted-foreground">{t(`rentalAudit.${field}`)}</dt><dd className="mt-0.5 text-sm tabular-nums">{formatWalletUnits(record.money![field], "$", locale)}</dd></div>)}{detail.settlement ? <div><dt className="text-xs text-muted-foreground">{t("rentalAudit.held")}</dt><dd className="text-sm tabular-nums">{formatWalletUnits(detail.settlement.heldUnits, "$", locale)}</dd></div> : null}</dl> : <p className="text-sm">{t("rentalAudit.moneyUnavailable")}</p>}
        </section>
        <details className="min-w-0"><summary className="cursor-pointer rounded text-sm font-semibold focus-visible:outline-2 focus-visible:outline-accent">{t("rentalAudit.periods")}</summary><div className="mt-3 grid gap-2">{!detail.periods.length ? <p className="text-xs text-muted-foreground">{t("rentalAudit.noPeriods")}</p> : detail.periods.map(period => <article key={period.id} className="grid gap-1 rounded-lg border border-border p-3 text-xs">
          <p className="font-medium">{t("rentalAudit.period", { sequence: period.sequence })} · {time(period.startedAt)} — {time(period.endedAt)}</p>
          <dl className="grid grid-cols-2 gap-2 sm:grid-cols-4">{([['chargedUnits', period.amountUnits], ['serviceUnits', period.serviceUnits], ['returnedUnits', period.returnedUnits], ['compensationUnits', period.compensationUnits]] as const).map(([key, value]) => <div key={key}><dt className="text-muted-foreground">{t(`rentalAudit.${key}`)}</dt><dd className="mt-1 tabular-nums">{formatWalletUnits(value, "$", locale)}</dd></div>)}</dl>
          <p className="text-muted-foreground">{t("rentalAudit.periodTerms", { commission: period.commissionRateBps === null ? "—" : period.commissionRateBps / 100, termination: period.terminationRateBps === null ? "—" : period.terminationRateBps / 100 })}</p>
          <p className="break-all text-muted-foreground">{t("rentalAudit.journal")}: {period.journalId}</p>
        </article>)}{detail.periodsNextCursor !== null ? <Button size="sm" variant="outline" isDisabled={busy} onClick={() => void load("periods")}>{t("rentalAudit.morePeriods")}</Button> : null}</div></details>
        <details className="min-w-0"><summary className="cursor-pointer rounded text-sm font-semibold focus-visible:outline-2 focus-visible:outline-accent">{t("rentalAudit.events")}</summary><div className="mt-3 grid gap-2"><ol className="grid gap-2">{detail.events.map(event => {
          const key = `rentalAudit.event.${event.action}`;
          return <li key={event.id} className="rounded-lg border border-border p-2 text-xs"><p className="font-medium">{t(key in rentalAuditMessagesEn ? key as MessageKey : "rentalAudit.event.updated")}</p><p className="mt-1 text-muted-foreground">{time(event.createdAt)}{event.actorEmail ? ` · ${event.actorEmail}` : ""}</p>{event.note ? <p className="mt-1 whitespace-pre-wrap break-words">{event.note}</p> : null}</li>;
        })}</ol>{detail.eventsNextCursor ? <Button size="sm" variant="outline" isDisabled={busy} onClick={() => void load("events")}>{t("rentalAudit.moreEvents")}</Button> : null}</div></details>
      </> : null}
    {record?.terminationId ? <div><Button size="sm" variant="outline" onPress={() => onTerminationOpen(record.terminationId!)}>{t("clientTermination.rowHistory")}</Button></div> : null}
  </section>;
}
