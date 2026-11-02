"use client";
import * as React from "react";
import { Button, Modal } from "@heroui/react";
import { useAuth } from "@/components/auth/auth-provider";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { CompactSelect } from "@/components/common/compact-select";
import { useClientChat } from "@/components/chat/client-chat";
import { WALLET_INPUT } from "./wallet-primitives";
import { formatWalletUnits } from "@/lib/market-wallet-money";
import { notifyMarketBalanceChanged } from "@/lib/market-balance-events";
import { parseTerminationAward } from "@/lib/client-termination";
import { actOnShareTermination, getShareTermination, getShareTerminationEvents, getShareTerminations, previewShareTermination, terminateShare, shareTerminationNoteValid, type ShareTermination, type ShareTerminationEvent, type ShareTerminationPreview, type ShareTerminationReason } from "@/lib/share-termination";

export type ShareTerminationSelection = { id: string } | { subscriptionId: string };
const failure = (e: unknown) => e instanceof Error ? e.message : String(e);
export function ShareTerminationDialog({ selection, onClose, onChanged }: { selection: ShareTerminationSelection | null; onClose: () => void; onChanged?: () => void }) {
  const { session } = useAuth();
  const actor = session?.authenticated ? session.user?.id : null;
  if (!actor || !selection) return null;
  return <DialogContent key={`${actor}:${"id" in selection ? selection.id : selection.subscriptionId}`} actor={actor} selection={selection} onClose={onClose} onChanged={onChanged} />;
}
function DialogContent({ actor, selection, onClose, onChanged }: { actor: string; selection: ShareTerminationSelection; onClose: () => void; onChanged?: () => void }) {
  const { locale, t } = useLocaleText();
  const { openChat } = useClientChat();
  const [preview, setPreview] = React.useState<ShareTerminationPreview | null>(null);
  const [record, setRecord] = React.useState<ShareTermination | null>(null);
  const [reason, setReason] = React.useState<ShareTerminationReason>("voluntary");
  const [note, setNote] = React.useState("");
  const [award, setAward] = React.useState("");
  const [error, setError] = React.useState("");
  const [loading, setLoading] = React.useState(true);
  const [busy, setBusy] = React.useState(false);
  const [clock, setClock] = React.useState(Date.now());
  const received = React.useRef(Date.now());
  const mounted = React.useRef(true);
  const controller = React.useRef<AbortController | null>(null);
  const working = React.useRef(false);
  const dirty = React.useRef(false);
  const requiresReview = React.useRef(false);
  const callbacks = React.useRef({ onChanged, onClose }); callbacks.current = { onChanged, onClose };
  const [currentId, setCurrentId] = React.useState("id" in selection ? selection.id : null);
  const subscriptionId = "subscriptionId" in selection ? selection.subscriptionId : null;
  const attempt = React.useRef<{ previewId: string; reason: ShareTerminationReason; claim: string } | null>(null);
  const load = React.useCallback(async (silent = false) => {
    if (working.current || silent && requiresReview.current) return;
    controller.current?.abort(); const c = new AbortController(); controller.current = c;
    if (!silent) { setLoading(true); setError(""); }
    try {
      const v = currentId ? await getShareTermination(currentId, c.signal) : await previewShareTermination(subscriptionId || "", c.signal);
      if (c.signal.aborted || !mounted.current) return;
      if ("permissions" in v) setRecord(v); else { setPreview(v); attempt.current = null; }
      requiresReview.current = false; dirty.current = false; setError("");
      received.current = Date.now(); setClock(Date.now());
    } catch (e) { if (!c.signal.aborted && mounted.current) setError(failure(e)); }
    finally { if (!c.signal.aborted && mounted.current) setLoading(false); }
  }, [currentId, subscriptionId]);
  React.useEffect(() => {
    mounted.current = true; void load();
    const timer = window.setInterval(() => {
      setClock(Date.now());
      if (currentId && document.visibilityState === "visible" && !working.current && !dirty.current) void load(true);
    }, 15000);
    return () => { mounted.current = false; controller.current?.abort(); window.clearInterval(timer); };
  }, [load, currentId]);
  React.useEffect(() => { const timer = window.setInterval(() => setClock(Date.now()), 1000); return () => window.clearInterval(timer); }, []);
  const serverNow = Date.parse(record?.serverNow || preview?.serverNow || "") + Math.max(0, clock - received.current);
  const expired = preview ? serverNow >= Date.parse(preview.expiresAt) : false;
  const due = record?.responseDeadlineAt ? serverNow >= Date.parse(record.responseDeadlineAt) : false;
  const money = (u: string) => formatWalletUnits(u, "$", locale);
  const mutate = async (action?: "respond" | "waive" | "resolve" | "withdraw" | "retry-revoke", units?: string) => {
    if (working.current || loading) return;
    if (!record && (!preview || expired && !attempt.current)) return;
    if (!shareTerminationNoteValid(note)) { setError(t("shareTermination.noteTooLong")); return; }
    controller.current?.abort(); const c = new AbortController(); controller.current = c;
    working.current = true; setBusy(true); setError("");
    try {
      let v: ShareTermination;
      if (record && action) v = await actOnShareTermination(record, action, note, units, c.signal);
      else {
        if (!("subscriptionId" in selection) || !preview) return;
        attempt.current ??= { previewId: preview.previewId, reason, claim: note };
        v = await terminateShare(selection.subscriptionId, attempt.current.previewId, attempt.current.reason, attempt.current.claim, c.signal);
      }
      if (!mounted.current || c.signal.aborted) return;
      setRecord(v); setPreview(null); setCurrentId(v.id); setNote(""); setAward(""); dirty.current = false;
      received.current = Date.now(); setClock(Date.now()); notifyMarketBalanceChanged(actor); callbacks.current.onChanged?.();
    } catch (e) {
      if (mounted.current && !c.signal.aborted) {
        setError(failure(e));
        // On version conflicts, keep the edited input but disable decisions
        // until the user explicitly reloads and reviews the current record.
        setRecord(null);
        if (record) { setCurrentId(record.id); requiresReview.current = true; dirty.current = true; }
      }
    } finally { working.current = false; if (mounted.current) setBusy(false); }
  };
  const decide = (kind: "waive" | "resolve") => {
    const units = parseTerminationAward(award);
    const cap = record ? BigInt(record.revokedAt ? record.heldUnits : record.compensationCapUnits) : 0n;
    if (!units || BigInt(units) > cap || kind === "waive" && BigInt(units) === cap) { setError(t("shareTermination.invalidAmount")); return; }
    if ((kind === "resolve" || BigInt(units) > 0n) && Array.from(note.trim()).length < 3) { setError(t("shareTermination.explanationRequired")); return; }
    void mutate(kind, units);
  };
  const close = () => { if (!working.current) callbacks.current.onClose(); };
  const p = record?.permissions;
  return <Modal.Backdrop isOpen onOpenChange={open => !open && close()}><Modal.Container>
    <Modal.Dialog className="light w-[min(600px,calc(100vw-2rem))] !bg-white !text-slate-900">
      <Modal.Header><Modal.Heading>{t(record ? "shareTermination.details" : "shareTermination.title")}</Modal.Heading></Modal.Header>
      <Modal.Body className="grid max-h-[72vh] min-w-0 gap-4 overflow-y-auto overscroll-contain">
        {loading ? <p role="status">{t("common.loading")}</p> : null}
        {error ? <p role="alert" className="break-words text-sm text-danger">{error}</p> : null}
        {preview ? <>
          <p className="text-sm leading-6 text-muted-foreground">{t("shareTermination.help")}</p>
          <dl className="grid grid-cols-2 gap-2 text-sm tabular-nums">{([
            ["shareTermination.cycle", preview.periodAmountUnits], ["shareTermination.remaining", preview.remainingUnits],
            ["shareTermination.cap", preview.compensationCapUnits], ["shareTermination.return", preview.returnUnits],
          ] as const).map(([key, units]) => <React.Fragment key={key}><dt className="text-muted-foreground">{t(key)}</dt><dd className="text-right">{money(units)}</dd></React.Fragment>)}</dl>
          {preview.usagePending ? <p className="text-xs leading-5 text-amber-800">{t("shareTermination.estimate")}</p> : null}
          <CompactSelect ariaLabel={t("shareTermination.reason")} value={reason} onChange={v => setReason(v as ShareTerminationReason)} disabled={busy || !!attempt.current} options={(["voluntary", "provider_issue", "platform_issue"] as const).map(value => ({ value, label: t(`shareTermination.${value}`) }))} />
          {reason !== "voluntary" ? <label className="grid gap-1 text-sm">{t("shareTermination.note")}<textarea className={WALLET_INPUT} value={note} maxLength={1500} disabled={busy || !!attempt.current} onChange={e => setNote(e.target.value)} /></label> : null}
          <details className="text-xs leading-6 text-muted-foreground"><summary className="cursor-pointer">{t("shareTermination.rules")}</summary>{t("shareTermination.terms", { rate: preview.rateBps / 100 })}</details>
          {expired ? <p className="text-sm text-amber-800">{t("shareTermination.expired")}</p> : null}
          {attempt.current && error ? <p className="text-xs text-amber-800">{t("shareTermination.unknown")}</p> : null}
          <Button variant="danger" isDisabled={busy || loading || expired && !attempt.current} onPress={() => void mutate()}>{t(attempt.current && error ? "shareTermination.retrySubmit" : "shareTermination.confirm")}</Button>
        </> : null}
        {record ? <>
          <p className="text-sm font-medium">{record.shareName || record.shareId}</p>
          <p role="status" className="text-sm text-muted-foreground">{t(record.status === "revoke_pending" ? "shareTermination.processing" : `shareTermination.${record.status}`)}</p>
          <dl className="grid grid-cols-2 gap-2 text-sm tabular-nums">{([
            ["shareTermination.returned", record.returnedUnits], ["shareTermination.quota", record.quotaEarnedUnits],
            ["shareTermination.awarded", record.compensationUnits], ["shareTermination.held", record.heldUnits],
          ] as const).map(([key, units]) => <React.Fragment key={key}><dt className="text-muted-foreground">{t(key)}</dt><dd className="text-right">{money(units)}</dd></React.Fragment>)}</dl>
          {record.claim ? <p className="whitespace-pre-wrap break-words text-sm">{record.claim}</p> : null}
          {record.decisionNote ? <p className="whitespace-pre-wrap break-words text-sm text-muted-foreground">{record.decisionNote}</p> : null}
          {record.responseDeadlineAt ? <p className="text-xs text-muted-foreground">{t("shareTermination.deadline")}: {new Date(record.responseDeadlineAt).toLocaleString(locale)}{due ? ` · ${t("shareTermination.timeout")}` : ""}</p> : null}
          {record.reason !== "voluntary" ? <div className="grid gap-1"><Button variant="outline" onPress={() => void openChat(record.installationId).catch(e => setError(failure(e)))}>{t("shareTermination.chat")}</Button><p className="text-xs text-muted-foreground">{t("shareTermination.chatHelp")}</p></div> : null}
          {(p?.respond || p?.resolve || p?.waive) && !due ? <label className="grid gap-1 text-sm">{t("shareTermination.responseNote")}<textarea className={WALLET_INPUT} maxLength={1500} value={note} disabled={busy} onChange={e => { dirty.current = true; setNote(e.target.value); }} /></label> : null}
          <div className="flex flex-wrap gap-2">
            {p?.waive && !due ? <Button variant="outline" isDisabled={busy} onPress={() => void mutate("waive", "0")}>{t("shareTermination.waive")}</Button> : null}
            {p?.respond && !due ? <Button variant="outline" isDisabled={busy || Array.from(note.trim()).length < 3} onPress={() => void mutate("respond")}>{t("shareTermination.respond")}</Button> : null}
            {p?.resolve && !due ? <Button variant="outline" isDisabled={busy || Array.from(note.trim()).length < 3} onPress={() => void mutate("resolve", record.revokedAt ? record.heldUnits : record.compensationCapUnits)}>{t("shareTermination.awardRemaining")}</Button> : null}
            {p?.retryRevoke ? <Button variant="outline" isDisabled={busy} onPress={() => void mutate("retry-revoke")}>{t("shareTermination.retry")}</Button> : null}
          </div>
          {(p?.waive || p?.resolve) && !due ? <div className="flex flex-wrap items-end gap-2"><label className="grid min-w-0 flex-1 gap-1 text-sm">{t("shareTermination.amount")}<input className={WALLET_INPUT} inputMode="decimal" value={award} disabled={busy} onChange={e => { dirty.current = true; setAward(e.target.value); }} /></label><Button variant="primary" isDisabled={busy || !award || Array.from(note.trim()).length < 3} onPress={() => decide(p.resolve ? "resolve" : "waive")}>{t(p.resolve ? "shareTermination.resolve" : "shareTermination.reduce")}</Button></div> : null}
          {p?.withdraw && !due ? <details className="text-xs text-muted-foreground"><summary className="cursor-pointer py-2">{t("shareTermination.withdraw")}</summary><p className="py-2">{t("shareTermination.withdrawHelp")}</p><Button variant="outline" isDisabled={busy} onPress={() => void mutate("withdraw")}>{t("shareTermination.withdraw")}</Button></details> : null}
          <SettlementHistory key={record.id} record={record} />
        </> : null}
      </Modal.Body>
      <Modal.Footer><Button variant="ghost" isDisabled={busy || loading} onPress={() => { dirty.current = false; void load(); }}>{t("common.reload")}</Button><Button variant="secondary" isDisabled={busy} onPress={close}>{t("common.close")}</Button></Modal.Footer>
    </Modal.Dialog>
  </Modal.Container></Modal.Backdrop>;
}

function SettlementHistory({ record }: { record: ShareTermination }) {
  const { t, locale } = useLocaleText();
  const [older, setOlder] = React.useState<ShareTerminationEvent[]>([]);
  const [cursor, setCursor] = React.useState(record.eventsNextCursor);
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState("");
  const controller = React.useRef<AbortController | null>(null);
  const loaded = React.useRef(false);
  React.useEffect(() => () => controller.current?.abort(), []);
  React.useEffect(() => { if (!loaded.current) setCursor(record.eventsNextCursor); }, [record.eventsNextCursor]);
  const more = async () => {
    if (!cursor || controller.current) return;
    const c = new AbortController(); controller.current = c; setBusy(true); setError("");
    try {
      const v = await getShareTerminationEvents(record.id, cursor, c.signal);
      if (c.signal.aborted) return;
      loaded.current = true; setOlder(old => [...old, ...v.items]); setCursor(v.nextCursor);
    } catch (e) { if (!c.signal.aborted) setError(failure(e)); }
    finally { controller.current = null; if (!c.signal.aborted) setBusy(false); }
  };
  const events = [...new Map([...older, ...record.events].map(e => [e.id, e])).values()]
    .sort((a, b) => b.createdAt.localeCompare(a.createdAt) || b.id.localeCompare(a.id));
  const labels: Record<string, Parameters<typeof t>[0]> = {
    accepted: "shareTermination.eventAccepted", settled: "shareTermination.eventSettled",
    return_unused: "shareTermination.eventReturn", return_compensation: "shareTermination.eventReturn",
    share_termination: "shareTermination.eventAward", automatic_credit: "shareTermination.eventCredit",
    respond: "shareTermination.eventRespond", waive: "shareTermination.eventWaive",
    resolve: "shareTermination.eventResolve", withdraw: "shareTermination.eventWithdraw", timeout: "shareTermination.eventTimeout",
  };
  return <details className="text-xs text-muted-foreground"><summary className="cursor-pointer py-2">{t("shareTermination.history")}</summary>
    <ol className="grid gap-2">{events.map(e => <li key={e.id}>
      <time>{new Date(e.createdAt).toLocaleString(locale)}</time> · {t(labels[e.action] || "shareTermination.eventUpdated")}
      {typeof e.detail.note === "string" && e.detail.note ? <p className="whitespace-pre-wrap break-words">{e.detail.note}</p> : null}
    </li>)}</ol>
    {error ? <p role="alert" className="py-2 text-danger">{error}</p> : null}
    {cursor ? <Button size="sm" variant="ghost" isDisabled={busy} onPress={() => void more()}>{t(busy ? "common.loading" : "shareTermination.moreHistory")}</Button> : null}
  </details>;
}

/** Only actionable, party-scoped rows get a red processing button. No queue
 * banner or permanent "view termination" button is added to every resource. */
export function ShareTerminationRowAction({ shareId, seatId }: { shareId: string; seatId?: string }) {
  const { session } = useAuth(); const { t } = useLocaleText();
  const actor = session?.authenticated ? session.user?.id : null;
  const identity = JSON.stringify([actor, shareId, seatId]);
  const [data, setData] = React.useState<{ identity: string; items: ShareTermination[] } | null>(null);
  const [selected, setSelected] = React.useState<{ identity: string; id: string } | null>(null);
  const [error, setError] = React.useState(""); const [revision, refresh] = React.useReducer(n => n + 1, 0);
  React.useEffect(() => { setData(null); setSelected(null); setError(""); }, [actor, shareId, seatId]);
  React.useEffect(() => {
    if (!actor) return;
    let live = true; let c: AbortController | null = null;
    const load = async () => {
      if (document.visibilityState !== "visible") return;
      c?.abort(); c = new AbortController(); const current = c;
      try { const v = await getShareTerminations({ shareId, seatId, actionOnly: true }, current.signal); if (live && !current.signal.aborted) { setData({ identity, items: v.items.filter(i => i.actionRequired) }); setError(""); } }
      catch (e) { if (live && !current.signal.aborted) { setData(null); setError(failure(e)); } }
    };
    void load(); const timer = window.setInterval(() => void load(), 15000);
    return () => { live = false; c?.abort(); window.clearInterval(timer); };
  }, [actor, shareId, seatId, revision, identity]);
  const items = data && data.identity === identity ? data.items : [];
  return <div data-no-card-open className="flex flex-wrap gap-1">
    {items.map(i => <Button key={i.id} size="sm" variant="danger" className="min-h-9 whitespace-nowrap text-xs" onPress={() => setSelected({ identity, id: i.id })}>{t(i.permissions.resolve ? "shareTermination.adjudicate" : "shareTermination.handle")}{items.length > 1 ? ` · ${i.buyerEmail}` : ""}</Button>)}
    {error ? <span title={error}><Button variant="ghost" size="sm" onPress={() => refresh()}>{t("shareTermination.unavailable")}</Button></span> : null}
    <ShareTerminationDialog selection={selected?.identity === identity ? { id: selected.id } : null} onClose={() => setSelected(null)} onChanged={() => refresh()} />
  </div>;
}
