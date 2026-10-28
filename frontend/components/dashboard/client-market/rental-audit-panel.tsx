"use client";

import * as React from "react";
import { Button, Modal } from "@heroui/react";
import { ChevronLeft, ChevronRight, Loader2, RefreshCw, Search } from "lucide-react";
import { useAuth } from "@/components/auth/auth-provider";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { ApiError } from "@/lib/api";
import { AUDIT_ATTENTION, AUDIT_MONEY_FIELDS, AUDIT_STATUSES, auditFiltersFromDates, getRentalAudit, getRentalAuditDetail, type AuditDetail, type AuditFilters, type AuditPage, type AuditRecord } from "@/lib/client-rental-audit";
import { formatWalletUnits } from "@/lib/market-wallet-money";
import { rentalAuditMessagesEn } from "@/lib/client-rental-audit-messages";
import type { MessageKey } from "@/lib/i18n";

const FIELD = "h-9 w-full min-w-0 rounded-lg border border-border bg-white px-2 text-sm text-foreground outline-none focus-visible:ring-2 focus-visible:ring-accent";
const INITIAL = { search: "", buyerEmail: "", providerEmail: "", status: "", attention: "", startDate: "", endDate: "" };
function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return <label className="grid min-w-0 gap-1 text-xs text-muted-foreground"><span>{label}</span>{children}</label>;
}

/** Separate from public Host filtering/polling. Unmount on capability/account
 * changes so no previous owner's emails or money remain in another session. */
export function ClientRentalAuditPanel({ onTerminationOpen, revision = 0 }: { onTerminationOpen: (id: string) => void; revision?: number }) {
  const { session } = useAuth();
  const actor = session?.authenticated && session.canAuditClientMarket === true ? session.user?.id : undefined;
  return actor ? <AuditWorkspace key={actor} onTerminationOpen={onTerminationOpen} revision={revision} /> : null;
}

function AuditWorkspace({ onTerminationOpen, revision }: { onTerminationOpen: (id: string) => void; revision: number }) {
  const { locale, t } = useLocaleText();
  const [draft, setDraft] = React.useState(INITIAL);
  const [filters, setFilters] = React.useState<AuditFilters>({});
  const [cursors, setCursors] = React.useState<(string | undefined)[]>([undefined]);
  const [refresh, setRefresh] = React.useState(0);
  const [loaded, setLoaded] = React.useState<{ scope: string; page: AuditPage } | null>(null);
  const [loading, setLoading] = React.useState(true);
  const [error, setError] = React.useState<"error" | "invalidDates" | null>(null);
  const [selected, setSelected] = React.useState<string | null>(null);
  const cursor = cursors[cursors.length - 1];
  const scope = JSON.stringify([filters, cursor]);
  const page = loaded?.scope === scope ? loaded.page : null;
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
  const time = (value: string | null) => value ? new Date(value).toLocaleString(locale) : "—";
  return <section aria-labelledby="client-rental-audit-title" className="grid min-w-0 gap-3 rounded-xl border border-border bg-card p-3 sm:p-4" data-testid="client-rental-audit">
    <div className="flex flex-wrap items-center justify-between gap-2">
      <div><h2 id="client-rental-audit-title" className="text-base font-semibold">{t("rentalAudit.title")}</h2><p className="text-xs text-muted-foreground">{t("rentalAudit.ownerOnly")}</p></div>
      <Button size="sm" variant="outline" isDisabled={loading} onClick={() => setRefresh(n => n + 1)}><RefreshCw className="h-3.5 w-3.5" />{t("rentalAudit.refresh")}</Button>
    </div>
    <p className="text-xs leading-relaxed text-muted-foreground">{t("rentalAudit.help")}</p>
    <form onSubmit={apply} className="grid gap-2 sm:grid-cols-2 xl:grid-cols-4">
      <Field label={t("rentalAudit.search")}><input className={FIELD} maxLength={254} value={draft.search} placeholder={t("rentalAudit.searchHint")} onChange={e => setDraft(v => ({ ...v, search: e.target.value }))} /></Field>
      <Field label={t("rentalAudit.buyer")}><input className={FIELD} maxLength={254} value={draft.buyerEmail} onChange={e => setDraft(v => ({ ...v, buyerEmail: e.target.value }))} /></Field>
      <Field label={t("rentalAudit.provider")}><input className={FIELD} maxLength={254} value={draft.providerEmail} onChange={e => setDraft(v => ({ ...v, providerEmail: e.target.value }))} /></Field>
      <Field label={t("rentalAudit.status")}><select className={FIELD} value={draft.status} onChange={e => setDraft(v => ({ ...v, status: e.target.value }))}>
        <option value="">{t("rentalAudit.all")}</option>{AUDIT_STATUSES.map(s => <option key={s} value={s}>{t(`rentalAudit.status.${s}`)}</option>)}
      </select></Field>
      <Field label={t("rentalAudit.attention")}><select className={FIELD} value={draft.attention} onChange={e => setDraft(v => ({ ...v, attention: e.target.value }))}>
        <option value="">{t("rentalAudit.all")}</option><option value="any">{t("rentalAudit.anyAttention")}</option>{AUDIT_ATTENTION.map(s => <option key={s} value={s}>{t(`rentalAudit.attention.${s}`)}</option>)}
      </select></Field>
      <Field label={t("rentalAudit.from")}><input type="date" className={FIELD} value={draft.startDate} onChange={e => setDraft(v => ({ ...v, startDate: e.target.value }))} /></Field>
      <Field label={t("rentalAudit.until")}><input type="date" className={FIELD} value={draft.endDate} onChange={e => setDraft(v => ({ ...v, endDate: e.target.value }))} /></Field>
      <div className="flex items-end gap-2"><Button size="sm" variant="primary" type="submit"><Search className="h-3.5 w-3.5" />{t("rentalAudit.apply")}</Button><Button size="sm" variant="ghost" type="button" onClick={() => { setDraft(INITIAL); setFilters({}); setCursors([undefined]); setRefresh(n => n + 1); }}>{t("common.reset")}</Button></div>
    </form>
    {error ? <p role="alert" className="text-sm text-destructive">{t(`rentalAudit.${error}`)}</p> : null}
    {loading ? <p role="status" className="flex items-center gap-2 text-xs text-muted-foreground"><Loader2 className="h-3.5 w-3.5 animate-spin" />{t("rentalAudit.loading")}</p> : null}
    {page ? <>
      <p className="text-xs text-muted-foreground xl:hidden">{t("rentalAudit.scrollHint")}</p>
      <div className="max-h-[32rem] min-w-0 overflow-auto rounded-lg border border-border" tabIndex={0} role="region" aria-label={t("rentalAudit.title")}>
        <table className="w-full min-w-[1060px] text-left text-xs">
          <thead className="sticky top-0 z-10 bg-muted"><tr>{["client", "parties", "time", "status", "money", "attention"].map(k => <th key={k} className="px-3 py-2 font-medium">{t(`rentalAudit.${k}` as MessageKey)}</th>)}<th className="px-3 py-2">{t("common.actions")}</th></tr></thead>
          <tbody>{page.items.map(record => <tr key={record.id} className="border-t border-border align-top">
            <td className="max-w-52 px-3 py-2"><p className="break-all font-medium">{record.clientSubdomain || record.installationId || record.jobId || record.id}</p><p className="mt-1 break-all text-muted-foreground">{record.hostIp ? `${record.hostIp}${record.hostPort ? `:${record.hostPort}` : ""}` : record.hostId || "—"}</p>{!record.hostPresent && record.hostId ? <p className="text-muted-foreground">{t("rentalAudit.removed")}</p> : null}</td>
            <td className="max-w-60 px-3 py-2"><p className="break-all">{record.buyerEmail || t("rentalAudit.notRecorded")}</p><p className="mt-1 break-all text-muted-foreground">{record.providerEmail || t("rentalAudit.notRecorded")}</p></td>
            <td className="whitespace-nowrap px-3 py-2"><p>{t("rentalAudit.orderedShort")}: {time(record.createdAt)}</p><p className="mt-1 text-muted-foreground">{record.activatedAt ? `${t("rentalAudit.activatedShort")}: ${time(record.activatedAt)}` : t("rentalAudit.notStarted")}</p>{record.endedAt ? <p className="mt-1 text-muted-foreground">{t("rentalAudit.endedShort")}: {time(record.endedAt)}</p> : record.activatedAt && record.status !== "ended" ? <p className="mt-1 text-muted-foreground">{t("rentalAudit.ongoing")}</p> : null}</td>
            <td className="px-3 py-2"><p className="font-medium">{t(`rentalAudit.status.${record.status}`)}</p>{record.cleanupStatus !== "not_required" ? <p className="mt-1 text-muted-foreground">{t(`rentalAudit.cleanup.${record.cleanupStatus}`)}</p> : null}</td>
            <td className="whitespace-nowrap px-3 py-2 tabular-nums">{record.money ? <><p>{formatWalletUnits(record.money.chargedUnits, "$", locale)}</p><p className="mt-1 text-muted-foreground">{formatWalletUnits(record.money.returnedUnits, "$", locale)}</p></> : <span title={t("rentalAudit.moneyUnavailable")}>—</span>}</td>
            <td className={`max-w-44 px-3 py-2 ${record.attention === "none" ? "text-muted-foreground" : "text-amber-700"}`}>{t(`rentalAudit.attention.${record.attention}`)}</td>
            <td className="px-3 py-2"><div className="grid justify-items-start gap-1"><Button size="sm" variant="ghost" onClick={() => setSelected(record.id)}>{t("rentalAudit.view")}</Button>{record.terminationId ? <Button size="sm" variant="outline" onClick={() => onTerminationOpen(record.terminationId!)}>{t("clientTermination.rowHistory")}</Button> : null}</div></td>
          </tr>)}</tbody>
        </table>
        {!page.items.length ? <p className="p-5 text-sm text-muted-foreground">{t("rentalAudit.empty")}</p> : null}
      </div>
      <div className="flex flex-wrap items-center justify-between gap-2 text-xs text-muted-foreground"><p>{t("rentalAudit.count", { count: page.total, page: cursors.length })}</p><div className="flex gap-1">
        <Button size="sm" variant="outline" isDisabled={loading || cursors.length === 1} onClick={() => setCursors(v => v.slice(0, -1))}><ChevronLeft className="h-3.5 w-3.5" />{t("common.previous")}</Button>
        <Button size="sm" variant="outline" isDisabled={loading || !page.nextCursor} onClick={() => { if (page.nextCursor) setCursors(v => [...v, page.nextCursor!]); }}>{t("common.next")}<ChevronRight className="h-3.5 w-3.5" /></Button>
      </div></div>
    </> : null}
    {selected ? <RentalDetail key={selected} id={selected} onClose={() => setSelected(null)} onTerminationOpen={id => { setSelected(null); onTerminationOpen(id); }} /> : null}
  </section>;
}

function RentalDetail({ id, onClose, onTerminationOpen }: { id: string; onClose: () => void; onTerminationOpen: (id: string) => void }) {
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
  React.useEffect(() => { void load(); return () => request.current?.abort(); }, [load]);
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
    [t("rentalAudit.buyer"), record.buyerEmail], [t("rentalAudit.provider"), record.providerEmail], ["IP", record.hostIp],
    [t("rentalAudit.created"), time(record.createdAt)], [t("rentalAudit.activated"), time(record.activatedAt)], [t("rentalAudit.ended"), time(record.endedAt)],
    [t("rentalAudit.cleanupAt"), time(record.cleanupConfirmedAt)], [t("rentalAudit.offer"), record.offerRevision?.toString()],
    [t("rentalAudit.price"), price], [t("rentalAudit.renewal"), renewal],
    [t("rentalAudit.trial"), record.trialSecondsRemaining === null ? "—" : `${detail?.trialConsumedSeconds ?? 0} / ${record.trialSecondsRemaining}`],
  ] : [];
  return <Modal.Backdrop isOpen onOpenChange={open => { if (!open) onClose(); }}><Modal.Container placement="center"><Modal.Dialog className="light max-h-[90dvh] w-[min(920px,calc(100vw-2rem))] max-w-none !bg-white !text-slate-900">
    <Modal.Header><Modal.Heading>{t("rentalAudit.details")}</Modal.Heading></Modal.Header>
    <Modal.Body className="grid gap-4 overflow-y-auto">
      {busy ? <p role="status" className="text-xs text-muted-foreground">{t("rentalAudit.loading")}</p> : null}
      {error ? <div role="alert" className="text-sm text-destructive">{t("rentalAudit.error")} <Button size="sm" variant="ghost" onClick={() => void load()}>{t("common.retry")}</Button></div> : null}
      {record && detail ? <>
        <div className="flex flex-wrap gap-2 text-xs"><span className="rounded bg-muted px-2 py-1">{t(`rentalAudit.status.${record.status}`)}</span><span className="rounded bg-muted px-2 py-1">{t(`rentalAudit.cleanup.${record.cleanupStatus}`)}</span><span className="rounded bg-muted px-2 py-1">{t(`rentalAudit.attention.${record.attention}`)}</span></div>
        <dl className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">{info.map(([label, value]) => <div key={label}><dt className="text-xs text-muted-foreground">{label}</dt><dd className="mt-0.5 break-all text-sm">{value || t("rentalAudit.notRecorded")}</dd></div>)}</dl>
        <p className="text-xs text-muted-foreground">{t(record.provenance === "order_snapshot" ? "rentalAudit.snapshot" : "rentalAudit.historical")}</p>
        <section className="grid gap-3 rounded-lg border border-border bg-muted/20 p-3"><h3 className="text-sm font-semibold">{t("rentalAudit.money")}</h3><p className="text-xs leading-relaxed text-muted-foreground">{t("rentalAudit.financialHelp")}</p>
          {record.money ? <dl className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">{AUDIT_MONEY_FIELDS.map(field => <div key={field}><dt className="text-xs text-muted-foreground">{t(`rentalAudit.${field}`)}</dt><dd className="mt-0.5 text-sm tabular-nums">{formatWalletUnits(record.money![field], "$", locale)}</dd></div>)}{detail.settlement ? <div><dt className="text-xs text-muted-foreground">{t("rentalAudit.held")}</dt><dd className="text-sm tabular-nums">{formatWalletUnits(detail.settlement.heldUnits, "$", locale)}</dd></div> : null}</dl> : <p className="text-sm">{t("rentalAudit.moneyUnavailable")}</p>}
        </section>
        <section className="grid gap-2"><h3 className="text-sm font-semibold">{t("rentalAudit.periods")}</h3>{!detail.periods.length ? <p className="text-xs text-muted-foreground">{t("rentalAudit.noPeriods")}</p> : detail.periods.map(period => <article key={period.id} className="grid gap-1 rounded-lg border border-border p-3 text-xs">
          <p className="font-medium">{t("rentalAudit.period", { sequence: period.sequence })} · {time(period.startedAt)} — {time(period.endedAt)}</p>
          <dl className="grid grid-cols-2 gap-2 sm:grid-cols-4">{([['chargedUnits', period.amountUnits], ['serviceUnits', period.serviceUnits], ['returnedUnits', period.returnedUnits], ['compensationUnits', period.compensationUnits]] as const).map(([key, value]) => <div key={key}><dt className="text-muted-foreground">{t(`rentalAudit.${key}`)}</dt><dd className="mt-1 tabular-nums">{formatWalletUnits(value, "$", locale)}</dd></div>)}</dl>
          <p className="text-muted-foreground">{t("rentalAudit.periodTerms", { commission: period.commissionRateBps === null ? "—" : period.commissionRateBps / 100, termination: period.terminationRateBps === null ? "—" : period.terminationRateBps / 100 })}</p>
          <p className="break-all text-muted-foreground">{t("rentalAudit.journal")}: {period.journalId}</p>
        </article>)}{detail.periodsNextCursor !== null ? <Button size="sm" variant="outline" isDisabled={busy} onClick={() => void load("periods")}>{t("rentalAudit.morePeriods")}</Button> : null}</section>
        <section className="grid gap-2"><h3 className="text-sm font-semibold">{t("rentalAudit.events")}</h3><ol className="grid gap-2">{detail.events.map(event => {
          const key = `rentalAudit.event.${event.action}`;
          return <li key={event.id} className="rounded-lg border border-border p-2 text-xs"><p className="font-medium">{t(key in rentalAuditMessagesEn ? key as MessageKey : "rentalAudit.event.updated")}</p><p className="mt-1 text-muted-foreground">{time(event.createdAt)}{event.actorEmail ? ` · ${event.actorEmail}` : ""}</p>{event.note ? <p className="mt-1 whitespace-pre-wrap break-words">{event.note}</p> : null}</li>;
        })}</ol>{detail.eventsNextCursor ? <Button size="sm" variant="outline" isDisabled={busy} onClick={() => void load("events")}>{t("rentalAudit.moreEvents")}</Button> : null}</section>
      </> : null}
    </Modal.Body><Modal.Footer>{record?.terminationId ? <Button variant="primary" onClick={() => onTerminationOpen(record.terminationId!)}>{t("clientTermination.rowHistory")}</Button> : null}<Button variant="ghost" onClick={onClose}>{t("common.close")}</Button></Modal.Footer>
  </Modal.Dialog></Modal.Container></Modal.Backdrop>;
}
