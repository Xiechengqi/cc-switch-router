"use client";
/*
 * The owner-only rental audit both markets share: search, status filter, a
 * fixed five-row table with page jumps, a filter dialog and a detail dialog
 * with ledger money, paid periods and the event timeline. Each market passes a
 * config with its own fetchers, labels and record facts.
 *
 * The workspace reads only while mounted. Markets render it as a tab, so
 * leaving the tab unmounts it: in-flight reads are aborted and no previous
 * owner's emails or money stay in memory. It never touches browser storage.
 */
import * as React from "react";
import { Button, Modal } from "@heroui/react";
import { ChevronLeft, ChevronRight, Loader2, RefreshCw, RotateCcw, Search, SlidersHorizontal } from "lucide-react";
import { CompactSelect } from "@/components/common/compact-select";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { ApiError } from "@/lib/api";
import {
  AUDIT_MONEY_FIELDS,
  auditFiltersFromDates,
  parseAuditPageNumber,
  type AuditEvent,
  type AuditFilters,
  type AuditMoney,
  type AuditPeriod,
} from "@/lib/client-rental-audit";
import { formatWalletUnits } from "@/lib/market-wallet-money";

// Share Market toolbar sizing (CLAUDE.md "Dashboard Toolbars"): 36px controls.
const FIELD = "h-9 w-full min-w-0 rounded-md border border-slate-200 bg-white px-2.5 text-xs text-foreground shadow-sm outline-none hover:border-slate-300 focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary disabled:cursor-not-allowed disabled:opacity-50";
const ACTION = "h-9 min-w-9 shrink-0 whitespace-nowrap px-2 text-xs";
const INITIAL = { search: "", buyerEmail: "", providerEmail: "", status: "", attention: "", startDate: "", endDate: "" };

export type RentalAuditBaseRecord = {
  id: string;
  buyerEmail: string | null;
  status: string;
  attention: string;
  money: AuditMoney | null;
  createdAt: string;
  terminationId: string | null;
};
export type RentalAuditBasePage<R> = { items: R[]; total: number; page: number; pageSize: number; totalPages: number; serverNow: string };
export type RentalAuditBaseDetail<R> = {
  record: R;
  periods: AuditPeriod[];
  periodsNextCursor: number | null;
  events: AuditEvent[];
  eventsNextCursor: string | null;
  settlement: { heldUnits: string | null } | null;
};
type DetailCursor = { periodCursor?: number; eventCursor?: string };

export type RentalAuditConfig<R extends RentalAuditBaseRecord, D extends RentalAuditBaseDetail<R>> = {
  /** Unique per market so ids and test hooks never collide. */
  scope: string;
  statuses: readonly string[];
  attention: readonly string[];
  statusLabel: (value: string) => string;
  attentionLabel: (value: string) => string;
  eventLabel: (action: string) => string;
  searchHint: string;
  /** Header of the first column ("Client", "Share"). */
  subjectHeader: string;
  subject: (record: R) => string;
  list: (filters: AuditFilters, page: number, signal: AbortSignal) => Promise<RentalAuditBasePage<R>>;
  detail: (id: string, cursor: DetailCursor, signal: AbortSignal) => Promise<D>;
  /** Labelled identity, time and price facts shown in the detail grid. */
  facts: (detail: D) => Array<[string, React.ReactNode]>;
  /** Short status chips at the top of the detail. */
  badges: (record: R) => string[];
  /** Provenance note under the facts. */
  note: (detail: D) => string;
  /** Market-specific sections placed after the money section (e.g. Share trial quota). */
  extra?: (detail: D) => React.ReactNode;
};

function Field({ label, children, as: Tag = "label" }: { label: string; children: React.ReactNode; as?: "label" | "div" }) {
  return <Tag className="grid min-w-0 gap-1 text-xs text-muted-foreground"><span>{label}</span>{children}</Tag>;
}

export function RentalAuditWorkspace<R extends RentalAuditBaseRecord, D extends RentalAuditBaseDetail<R>>({
  config,
  onTerminationOpen,
  revision = 0,
}: {
  config: RentalAuditConfig<R, D>;
  onTerminationOpen?: (id: string) => void;
  revision?: number;
}) {
  const { t } = useLocaleText();
  const [draft, setDraft] = React.useState(INITIAL);
  const [filters, setFilters] = React.useState<AuditFilters>({});
  const [requestedPage, setRequestedPage] = React.useState(1);
  const [refresh, setRefresh] = React.useState(0);
  const [read, setRead] = React.useState<{ scope: string; page: RentalAuditBasePage<R> | null; state: "loading" | "ready" | "error" } | null>(null);
  const [selected, setSelected] = React.useState<{ scope: string; id: string } | null>(null);
  const [filtersOpen, setFiltersOpen] = React.useState(false);
  const [invalidDates, setInvalidDates] = React.useState(false);
  const [jump, setJump] = React.useState("1");
  const [invalidPage, setInvalidPage] = React.useState(false);
  const request = React.useRef<AbortController | null>(null);
  const formId = React.useId();
  const jumpErrorId = React.useId();
  const titleId = `${config.scope}-rental-audit-title`;
  const scope = JSON.stringify([filters, requestedPage, refresh]);
  const current = read?.scope === scope ? read : null;
  const page = current?.page ?? null;
  const loading = !current || current.state === "loading";
  const error = current?.state === "error";
  const selection = selected?.scope === scope ? selected : null;
  const list = config.list;
  const denyAccess = React.useCallback(() => {
    request.current?.abort();
    setRead({ scope, page: null, state: "error" });
    setSelected(null); setFiltersOpen(false);
  }, [scope]);
  React.useEffect(() => {
    const controller = new AbortController();
    request.current = controller;
    setRead({ scope, page: null, state: "loading" });
    void list(filters, requestedPage, controller.signal).then(value => {
      // Use the server's authoritative page after shrinking results. Keeping
      // the request key avoids a redundant second fetch just to clamp state.
      if (!controller.signal.aborted) setRead({ scope, page: value, state: "ready" });
    }).catch(error => {
      if (controller.signal.aborted) return;
      if (error instanceof ApiError && (error.status === 401 || error.status === 403)) { setSelected(null); setFiltersOpen(false); }
      setRead({ scope, page: null, state: "error" });
    });
    return () => controller.abort();
  }, [list, filters, requestedPage, refresh, revision, scope]);
  // Clear, rather than merely hide, the previous page's selection: returning
  // to that page must never reopen a stale detail dialog.
  React.useEffect(() => { setSelected(null); }, [scope]);
  React.useEffect(() => { setJump(String(page?.page ?? requestedPage)); setInvalidPage(false); }, [page?.page, requestedPage, scope]);
  const apply = (event: React.FormEvent) => {
    event.preventDefault();
    try {
      setFilters(auditFiltersFromDates(draft)); setRequestedPage(1); setRefresh(n => n + 1);
      setInvalidDates(false); setFiltersOpen(false);
    } catch { setInvalidDates(true); }
  };
  const reset = () => { setDraft(INITIAL); setFilters({}); setRequestedPage(1); setRefresh(n => n + 1); setInvalidDates(false); setFiltersOpen(false); };
  const jumpToPage = (event: React.FormEvent) => {
    event.preventDefault();
    if (loading || !page?.total) return;
    const number = parseAuditPageNumber(jump, page.totalPages);
    setInvalidPage(number === null);
    if (number !== null) { setJump(String(number)); setRequestedPage(number); }
  };
  const resetDisabled = !Object.values(draft).some(Boolean) && !Object.values(filters).some(Boolean);
  return <section aria-labelledby={titleId} className="grid h-[41.5rem] min-w-0 grid-rows-[2.25rem_7.5rem_1.25rem_19.75rem_minmax(0,1fr)] gap-3 rounded-xl border border-border bg-card p-3 sm:h-[35.5rem] sm:grid-rows-[2.25rem_3.5rem_1.25rem_19.75rem_minmax(0,1fr)] sm:p-4" data-testid={`${config.scope}-rental-audit`}>
    <div className="flex min-w-0 items-center justify-between gap-2">
      <div><h3 id={titleId} className="sr-only">{t("rentalAudit.tab")}</h3><p className="text-xs text-muted-foreground">{t("rentalAudit.ownerOnly")}</p></div>
      <Button size="sm" variant="outline" className={ACTION} aria-label={t("rentalAudit.refresh")} isDisabled={loading} onPress={() => { setRequestedPage(1); setRefresh(n => n + 1); }}><RefreshCw aria-hidden className="h-3.5 w-3.5" /><span className="hidden sm:inline">{t("rentalAudit.refresh")}</span></Button>
    </div>
    <form onSubmit={apply} className="grid min-w-0 grid-cols-[minmax(0,1fr)_auto] items-end gap-2 sm:grid-cols-[minmax(0,1fr)_minmax(0,0.65fr)_auto]">
      <div className="col-span-2 min-w-0 sm:col-span-1"><Field label={t("rentalAudit.search")}><input className={FIELD} maxLength={254} value={draft.search} placeholder={config.searchHint} onChange={e => setDraft(v => ({ ...v, search: e.target.value }))} /></Field></div>
      <Field as="div" label={t("rentalAudit.status")}><CompactSelect ariaLabel={t("rentalAudit.status")} className="w-full" triggerClassName="h-9" value={draft.status} onChange={status => setDraft(v => ({ ...v, status }))}
        options={[{ value: "", label: t("rentalAudit.all") }, ...config.statuses.map(value => ({ value, label: config.statusLabel(value) }))]} /></Field>
      <div className="flex gap-1">
        <Button size="sm" variant="primary" className={ACTION} type="submit" aria-label={t("rentalAudit.apply")}><Search aria-hidden className="h-3.5 w-3.5" /><span className="hidden md:inline">{t("rentalAudit.apply")}</span></Button>
        <Button size="sm" variant="outline" className={ACTION} type="button" aria-haspopup="dialog" aria-label={t("rentalAudit.filters")} onPress={() => setFiltersOpen(true)}><SlidersHorizontal aria-hidden className="h-3.5 w-3.5" /><span className="hidden md:inline">{t("rentalAudit.filters")}</span></Button>
        <Button size="sm" variant="ghost" className={ACTION} type="button" aria-label={t("common.reset")} isDisabled={resetDisabled} onPress={reset}><RotateCcw aria-hidden className="h-3.5 w-3.5" /><span className="hidden lg:inline">{t("common.reset")}</span></Button>
      </div>
    </form>
    <div className="min-w-0 text-xs text-destructive">{invalidPage ? <p id={jumpErrorId} role="alert" className="truncate" title={t("rentalAudit.invalidPage", { pages: page?.totalPages ?? 1 })}>{t("rentalAudit.invalidPage", { pages: page?.totalPages ?? 1 })}</p> : invalidDates && !filtersOpen ? <p role="alert">{t("rentalAudit.invalidDates")}</p> : null}</div>
    <div className="min-w-0 rounded-lg border border-border" role="region" aria-label={t("rentalAudit.tab")} aria-busy={loading} data-testid="rental-audit-list">
      {loading ? <p role="status" className="flex h-full items-center justify-center gap-2 text-sm text-muted-foreground"><Loader2 aria-hidden className="h-4 w-4 animate-spin motion-reduce:animate-none" />{t("rentalAudit.loading")}</p>
        : error ? <div role="alert" className="grid h-full content-center justify-items-center gap-3 px-4 text-center text-sm"><p className="max-w-md text-destructive">{t("rentalAudit.error")}</p><Button variant="outline" onPress={() => setRefresh(n => n + 1)}>{t("common.retry")}</Button></div>
        : !page?.items.length ? <p className="grid h-full place-items-center p-4 text-sm text-muted-foreground">{t("rentalAudit.empty")}</p> :
        <table className="w-full table-fixed text-left text-xs">
          <thead className="bg-muted"><tr className="h-8">
            <th scope="col" className="px-2 font-medium sm:px-3">{config.subjectHeader}</th>
            <th scope="col" className="hidden px-3 font-medium md:table-cell">{t("rentalAudit.buyer")}</th>
            <th scope="col" className="w-24 px-2 font-medium sm:w-36 sm:px-3">{t("rentalAudit.status")}</th>
            <th scope="col" className="hidden w-24 px-2 text-right font-medium sm:table-cell">{t("rentalAudit.chargedShort")}</th>
            <th scope="col" className="hidden w-40 px-3 font-medium lg:table-cell">{t("rentalAudit.orderedShort")}</th>
            <th scope="col" className="w-10"><span className="sr-only">{t("rentalAudit.details")}</span></th>
          </tr></thead>
          <tbody>{page.items.map(record => <AuditRow key={record.id} config={config} record={record} onOpen={() => setSelected({ scope, id: record.id })} />)}</tbody>
        </table>}
    </div>
    <nav className="grid min-w-0 content-start grid-cols-2 gap-1 text-xs text-muted-foreground" aria-label={t("rentalAudit.pagination")}>
      <p className="col-span-2 truncate tabular-nums" aria-live="polite">{page ? t("rentalAudit.count", { count: page.total, page: page.page, pages: page.totalPages }) : "—"}</p>
      <div className="col-span-2 flex justify-between gap-1 sm:col-span-1 sm:justify-start">
        <Button size="sm" variant="outline" className={ACTION} isDisabled={loading || !page || page.page === 1} onPress={() => { if (page) setRequestedPage(page.page - 1); }}><ChevronLeft aria-hidden className="h-3.5 w-3.5" />{t("common.previous")}</Button>
        <Button size="sm" variant="outline" className={ACTION} isDisabled={loading || !page || page.page >= page.totalPages} onPress={() => { if (page) setRequestedPage(page.page + 1); }}>{t("common.next")}<ChevronRight aria-hidden className="h-3.5 w-3.5" /></Button>
      </div>
      <form className="col-span-2 flex min-w-0 items-center justify-between gap-2 sm:col-span-1 sm:justify-end" onSubmit={jumpToPage}>
        <label htmlFor={`${formId}-page`} className="whitespace-nowrap">{t("rentalAudit.pageNumber")}</label>
        <input id={`${formId}-page`} className={`${FIELD} max-w-20 tabular-nums ${invalidPage ? "border-destructive" : ""}`} inputMode="numeric" maxLength={10} value={jump} disabled={loading || !page?.total} aria-invalid={invalidPage} aria-describedby={invalidPage ? jumpErrorId : undefined} onChange={event => { setJump(event.target.value); setInvalidPage(false); }} />
        <Button size="sm" variant="outline" className={ACTION} type="submit" isDisabled={loading || !page?.total}>{t("rentalAudit.jump")}</Button>
      </form>
    </nav>
    {filtersOpen ? <AuditDialog title={t("rentalAudit.filters")} onClose={() => setFiltersOpen(false)} footer={<><Button variant="ghost" isDisabled={resetDisabled} onPress={reset}>{t("common.reset")}</Button><Button variant="primary" type="submit" form={formId}>{t("rentalAudit.apply")}</Button></>}>
      <form id={formId} onSubmit={apply} className="grid min-w-0 gap-3 sm:grid-cols-2">
        <Field label={t("rentalAudit.buyer")}><input className={FIELD} maxLength={254} value={draft.buyerEmail} onChange={e => setDraft(v => ({ ...v, buyerEmail: e.target.value }))} /></Field>
        <Field label={t("rentalAudit.provider")}><input className={FIELD} maxLength={254} value={draft.providerEmail} onChange={e => setDraft(v => ({ ...v, providerEmail: e.target.value }))} /></Field>
        <Field as="div" label={t("rentalAudit.attention")}><CompactSelect ariaLabel={t("rentalAudit.attention")} className="w-full" triggerClassName="h-9" value={draft.attention} onChange={attention => setDraft(v => ({ ...v, attention }))}
          options={[{ value: "", label: t("rentalAudit.all") }, { value: "any", label: t("rentalAudit.anyAttention") }, ...config.attention.map(value => ({ value, label: config.attentionLabel(value) }))]} /></Field>
        <Field label={t("rentalAudit.from")}><input type="date" className={FIELD} value={draft.startDate} onChange={e => setDraft(v => ({ ...v, startDate: e.target.value }))} /></Field>
        <Field label={t("rentalAudit.until")}><input type="date" className={FIELD} value={draft.endDate} onChange={e => setDraft(v => ({ ...v, endDate: e.target.value }))} /></Field>
        {invalidDates ? <p role="alert" className="text-sm text-destructive sm:col-span-2">{t("rentalAudit.invalidDates")}</p> : null}
      </form>
    </AuditDialog> : null}
    {selection ? <AuditDialog title={t("rentalAudit.details")} onClose={() => setSelected(null)}><RentalDetail key={selection.id} config={config} id={selection.id} revision={`${refresh}:${revision}`} onTerminationOpen={onTerminationOpen} onForbidden={denyAccess} /></AuditDialog> : null}
  </section>;
}

function AuditDialog({ title, onClose, children, footer }: { title: string; onClose: () => void; children: React.ReactNode; footer?: React.ReactNode }) {
  const { t } = useLocaleText();
  return <Modal.Backdrop isOpen onOpenChange={open => { if (!open) onClose(); }}>
    <Modal.Container placement="center"><Modal.Dialog className="light w-[min(52rem,calc(100%-2rem))] max-w-none bg-card text-foreground">
      <Modal.CloseTrigger />
      <Modal.Header><Modal.Heading>{title}</Modal.Heading></Modal.Header>
      <Modal.Body className="grid min-w-0 max-h-[65dvh] gap-4 overflow-y-auto overscroll-contain">{children}</Modal.Body>
      <Modal.Footer className="flex-wrap">{footer}<Button variant="outline" className="whitespace-nowrap" onPress={onClose}>{t("common.close")}</Button></Modal.Footer>
    </Modal.Dialog></Modal.Container>
  </Modal.Backdrop>;
}

function AuditRow<R extends RentalAuditBaseRecord, D extends RentalAuditBaseDetail<R>>({ config, record, onOpen }: { config: RentalAuditConfig<R, D>; record: R; onOpen: () => void }) {
  const { locale, t } = useLocaleText();
  const subject = config.subject(record);
  const buyer = record.buyerEmail || t("rentalAudit.notRecorded");
  const status = config.statusLabel(record.status);
  const attention = record.attention !== "none" ? config.attentionLabel(record.attention) : null;
  return <tr data-testid="rental-audit-row" className="h-14 cursor-pointer border-t border-border align-middle hover:bg-muted/50 focus-within:bg-muted/50"
      onClick={event => {
        if ((event.target as HTMLElement).closest("button, a") || window.getSelection()?.toString()) return;
        onOpen();
      }}>
      <td className="px-2 sm:px-3"><p className="truncate font-medium" title={subject}>{subject}</p><p className="mt-1 truncate text-muted-foreground md:hidden" title={buyer}>{buyer}</p></td>
      <td className="hidden px-3 md:table-cell"><p className="truncate" title={buyer}>{buyer}</p></td>
      <td className="px-2 sm:px-3"><p className="truncate" title={status}>{status}</p>{attention ? <p className="mt-1 truncate font-medium text-foreground" title={attention}>{attention}</p> : null}</td>
      <td className="hidden px-2 text-right tabular-nums sm:table-cell"><p className="truncate">{record.money ? formatWalletUnits(record.money.chargedUnits, "$", locale) : <span title={t("rentalAudit.moneyUnavailable")}>—</span>}</p></td>
      <td className="hidden px-3 text-muted-foreground lg:table-cell"><time className="block truncate" dateTime={record.createdAt}>{new Intl.DateTimeFormat(locale, { dateStyle: "short", timeStyle: "short" }).format(new Date(record.createdAt))}</time></td>
      <td><Button size="sm" variant="ghost" className="h-9 w-9" isIconOnly aria-label={t("rentalAudit.open", { client: subject })} aria-haspopup="dialog" onPress={onOpen}>
        <ChevronRight aria-hidden className="h-4 w-4" />
      </Button></td>
    </tr>;
}

function RentalDetail<R extends RentalAuditBaseRecord, D extends RentalAuditBaseDetail<R>>({ config, id, revision, onTerminationOpen, onForbidden }: {
  config: RentalAuditConfig<R, D>; id: string; revision: string; onTerminationOpen?: (id: string) => void; onForbidden: () => void;
}) {
  const { locale, t } = useLocaleText();
  const [detail, setDetail] = React.useState<D | null>(null);
  const [busy, setBusy] = React.useState(true);
  const [error, setError] = React.useState(false);
  const request = React.useRef<AbortController | null>(null);
  const forbidden = React.useRef(onForbidden); forbidden.current = onForbidden;
  const latest = React.useRef(detail); latest.current = detail;
  const fetchDetail = config.detail;
  const load = React.useCallback(async (kind: "initial" | "periods" | "events" = "initial") => {
    request.current?.abort(); const controller = new AbortController(); request.current = controller;
    setBusy(true); setError(false);
    const previous = latest.current;
    try {
      const next = await fetchDetail(id, kind === "periods" ? { periodCursor: previous?.periodsNextCursor ?? undefined } : kind === "events" ? { eventCursor: previous?.eventsNextCursor ?? undefined } : {}, controller.signal);
      if (controller.signal.aborted) return;
      setDetail(current => {
        if (!current || kind === "initial") return next;
        if (kind === "periods") return { ...next, periods: [...current.periods, ...next.periods.filter(p => !current.periods.some(old => old.id === p.id))], events: current.events, eventsNextCursor: current.eventsNextCursor };
        return { ...next, events: [...current.events, ...next.events.filter(e => !current.events.some(old => old.id === e.id))], periods: current.periods, periodsNextCursor: current.periodsNextCursor };
      });
    } catch (error) {
      if (!controller.signal.aborted) {
        if (error instanceof ApiError && (error.status === 401 || error.status === 403)) { setDetail(null); forbidden.current(); }
        setError(true);
      }
    }
    finally { if (!controller.signal.aborted) setBusy(false); }
  }, [fetchDetail, id]);
  React.useEffect(() => { void load(); return () => request.current?.abort(); }, [load, revision]);
  const time = (v: string | null) => v ? new Date(v).toLocaleString(locale) : "—";
  const record = detail?.record;
  return <section aria-label={t("rentalAudit.details")} className="grid min-w-0 gap-4" data-testid="rental-audit-detail">
      {busy ? <p role="status" className="text-xs text-muted-foreground">{t("rentalAudit.loading")}</p> : null}
      {error ? <div role="alert" className="text-sm text-destructive">{t("rentalAudit.error")} <Button size="sm" variant="ghost" onClick={() => void load()}>{t("common.retry")}</Button></div> : null}
      {record && detail ? <>
        <div className="flex flex-wrap gap-2 text-xs">{config.badges(record).map(badge => <span key={badge} className="rounded bg-muted px-2 py-1">{badge}</span>)}</div>
        <dl className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">{config.facts(detail).map(([label, value]) => <div key={label}><dt className="text-xs text-muted-foreground">{label}</dt><dd className="mt-0.5 break-all text-sm">{value || t("rentalAudit.notRecorded")}</dd></div>)}</dl>
        <p className="text-xs text-muted-foreground">{config.note(detail)}</p>
        <section className="grid gap-3 rounded-lg border border-border bg-muted/20 p-3"><h3 className="text-sm font-semibold">{t("rentalAudit.money")}</h3><p className="text-xs leading-relaxed text-muted-foreground">{t("rentalAudit.financialHelp")}</p>
          {record.money ? <dl className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">{AUDIT_MONEY_FIELDS.map(field => <div key={field}><dt className="text-xs text-muted-foreground">{t(`rentalAudit.${field}`)}</dt><dd className="mt-0.5 text-sm tabular-nums">{formatWalletUnits(record.money![field], "$", locale)}</dd></div>)}{detail.settlement?.heldUnits != null ? <div><dt className="text-xs text-muted-foreground">{t("rentalAudit.held")}</dt><dd className="text-sm tabular-nums">{formatWalletUnits(detail.settlement.heldUnits, "$", locale)}</dd></div> : null}</dl> : <p className="text-sm">{t("rentalAudit.moneyUnavailable")}</p>}
        </section>
        {config.extra?.(detail)}
        <details className="min-w-0"><summary className="cursor-pointer rounded text-sm font-semibold focus-visible:outline-2 focus-visible:outline-accent">{t("rentalAudit.periods")}</summary><div className="mt-3 grid gap-2">{!detail.periods.length ? <p className="text-xs text-muted-foreground">{t("rentalAudit.noPeriods")}</p> : detail.periods.map(period => <article key={period.id} className="grid gap-1 rounded-lg border border-border p-3 text-xs">
          <p className="font-medium">{t("rentalAudit.period", { sequence: period.sequence })} · {time(period.startedAt)} — {time(period.endedAt)}</p>
          <dl className="grid grid-cols-2 gap-2 sm:grid-cols-4">{([["chargedUnits", period.amountUnits], ["serviceUnits", period.serviceUnits], ["returnedUnits", period.returnedUnits], ["compensationUnits", period.compensationUnits]] as const).map(([key, value]) => <div key={key}><dt className="text-muted-foreground">{t(`rentalAudit.${key}`)}</dt><dd className="mt-1 tabular-nums">{formatWalletUnits(value, "$", locale)}</dd></div>)}</dl>
          <p className="text-muted-foreground">{t("rentalAudit.periodTerms", { commission: period.commissionRateBps === null ? "—" : period.commissionRateBps / 100, termination: period.terminationRateBps === null ? "—" : period.terminationRateBps / 100 })}</p>
          <p className="break-all text-muted-foreground">{t("rentalAudit.journal")}: {period.journalId}</p>
        </article>)}{detail.periodsNextCursor !== null ? <Button size="sm" variant="outline" isDisabled={busy} onClick={() => void load("periods")}>{t("rentalAudit.morePeriods")}</Button> : null}</div></details>
        <details className="min-w-0"><summary className="cursor-pointer rounded text-sm font-semibold focus-visible:outline-2 focus-visible:outline-accent">{t("rentalAudit.events")}</summary><div className="mt-3 grid gap-2"><ol className="grid gap-2">{detail.events.map(event => (
          <li key={event.id} className="rounded-lg border border-border p-2 text-xs"><p className="font-medium">{config.eventLabel(event.action)}</p><p className="mt-1 text-muted-foreground">{time(event.createdAt)}{event.actorEmail ? ` · ${event.actorEmail}` : ""}</p>{event.note ? <p className="mt-1 whitespace-pre-wrap break-words">{event.note}</p> : null}</li>
        ))}</ol>{detail.eventsNextCursor ? <Button size="sm" variant="outline" isDisabled={busy} onClick={() => void load("events")}>{t("rentalAudit.moreEvents")}</Button> : null}</div></details>
      </> : null}
    {record?.terminationId && onTerminationOpen ? <div><Button size="sm" variant="outline" onPress={() => onTerminationOpen(record.terminationId!)}>{t("clientTermination.rowHistory")}</Button></div> : null}
  </section>;
}
