"use client";
/* Hallmark · component: C2C orders and seller workspace · genre: modern-minimal · theme: existing Router tokens
 * states: default · hover · focus · active · disabled · loading · error · success
 * pre-emit critique: P5 H5 E4 S5 R5 V4
 */
import * as React from "react";
import { useSearchParams } from "next/navigation";
import { ChevronRight, RefreshCw } from "lucide-react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { C2CSellerPanel } from "./c2c-seller-panel";
import { C2CDescription } from "@/components/market/c2c-description";
import { C2CAction, C2CCheck, C2CDialog, C2CError, C2CStatusLabel, C2C_INPUT } from "@/components/market/c2c-primitives";
import { c2cErrorKey, c2cNow, c2cPayLabel, c2cPaymentWindow, getC2CPolicy, getC2CSeller, pauseC2CSeller, resumeC2CSeller, getC2COrders, getC2COrder, getC2CEvents, actC2C, releaseC2C, type C2CSeller, type C2CSellerSnapshot, type C2COrder, type C2CEvent, type C2CPolicyView } from "@/lib/market-c2c";
import { formatTopupCountdown, formatWalletUnits } from "@/lib/market-wallet-money";
import { notifyMarketBalanceChanged, subscribeMarketBalanceChanged } from "@/lib/market-balance-events";
import { messages, type MessageKey } from "@/lib/i18n";

export function C2CEvents({ items }: { items: C2CEvent[] }) {
  const { t, locale } = useLocaleText();
  return <ul className="grid min-w-0 gap-3">
    {items.map(e => { const key = `c2c.action.${e.action}` as MessageKey; return <li key={e.id} className="grid min-w-0 gap-1 text-xs"><span className="flex min-w-0 flex-wrap justify-between gap-2"><span>{key in messages.en ? t(key) : e.action}</span><time className="text-muted-foreground">{new Date(e.createdAt).toLocaleString(locale)}</time></span>{["note", "disputed", "late_payment_reported", "release", "return_to_seller", "record_external_resolution"].includes(e.action) && e.note ? <p className="whitespace-pre-wrap break-words text-muted-foreground [overflow-wrap:anywhere]">{e.note}</p> : null}</li>; })}
  </ul>;
}
export function C2COrdersPanel({ actor, selectedId, onSelect, embedded = false, sellOpen = false, onSellOpenChange }: {
  actor: string;
  selectedId: string | null;
  onSelect: (id: string | null) => void;
  embedded?: boolean;
  sellOpen?: boolean;
  onSellOpenChange?: (open: boolean) => void;
}) {
  return <C2COrdersWorkspace key={actor} actor={actor} selectedId={selectedId} onSelect={onSelect} embedded={embedded} sellOpen={sellOpen} onSellOpenChange={onSellOpenChange} />;
}
function C2COrdersWorkspace({ actor, selectedId, onSelect, embedded = false, sellOpen = false, onSellOpenChange }: {
  actor: string;
  selectedId: string | null;
  onSelect: (id: string | null) => void;
  embedded?: boolean;
  sellOpen?: boolean;
  onSellOpenChange?: (open: boolean) => void;
}) {
  const { t } = useLocaleText();
  const [policy, setPolicy] = React.useState<C2CPolicyView | null>(null);
  const [snapshot, setSnapshot] = React.useState<C2CSellerSnapshot | null>(null);
  const [items, setItems] = React.useState<C2COrder[]>([]);
  const [next, setNext] = React.useState<string | null>(null);
  const [history, setHistory] = React.useState(false);
  const [activeCount, setActiveCount] = React.useState(0);
  const params = useSearchParams();
  const sellOpenRef = React.useRef(sellOpen);
  const [selling, setSelling] = React.useState(sellOpen || params.get("c2cSell") === "1");
  const setSellOpen = (open: boolean) => { sellOpenRef.current = open; setSelling(open); onSellOpenChange?.(open); };
  React.useEffect(() => {
    if (sellOpen === sellOpenRef.current) return;
    sellOpenRef.current = sellOpen;
    setSelling(sellOpen);
  }, [sellOpen]);
  const [editorKey, setEditorKey] = React.useState(0);
  const [editorBusy, setEditorBusy] = React.useState(false);
  const [error, setError] = React.useState("");
  const [sellerError, setSellerError] = React.useState("");
  const [notice, setNotice] = React.useState("");
  const [loaded, setLoaded] = React.useState(false);
  const [busy, setBusy] = React.useState(false);
  const [pausing, setPausing] = React.useState(false);
  const alive = React.useRef(true); const refreshVersion = React.useRef(0);
  const settingsVersion = React.useRef(0); const pauseLock = React.useRef(false);
  const identity = React.useRef(actor); identity.current = actor;
  React.useEffect(() => { alive.current = true; return () => { alive.current = false; refreshVersion.current++; settingsVersion.current++; }; }, [actor]);
  const load = React.useCallback(async (before = "") => {
    const version = ++refreshVersion.current;
    try {
      const page = await getC2COrders({ status: history ? "" : "active", before });
      if (!alive.current || identity.current !== actor || refreshVersion.current !== version) return;
      if (page.items.some(o => o.buyerId !== actor && o.sellerId !== actor)) throw new Error("C2C account changed");
      setLoaded(true); setError(""); setNext(page.next);
      setItems(old => before ? [...old, ...page.items.filter(item => !old.some(v => v.id === item.id))] : page.items);
      if (!history) setActiveCount(page.items.length);
    } catch (cause) { if (alive.current && refreshVersion.current === version) throw cause; }
  }, [actor, history]);
  const loadSettings = React.useCallback(async () => {
    const version = ++settingsVersion.current;
    try {
      const [p, s] = await Promise.all([getC2CPolicy(), getC2CSeller()]);
      if (!alive.current || identity.current !== actor || settingsVersion.current !== version) return false;
      if (s.seller && s.seller.userId !== actor) throw new Error("C2C account changed");
      setPolicy(p); setSnapshot(s); setSellerError("");
      return true;
    } catch (cause) { if (alive.current && settingsVersion.current === version) throw cause; return false; }
  }, [actor]);
  React.useEffect(() => {
    setLoaded(false); setItems([]); void load().catch(e => { if (alive.current) setError(t(c2cErrorKey(e))); });
    const refresh = () => {
      if (document.visibilityState !== "visible") return;
      if (!history) void load().catch(e => { if (alive.current) setError(t(c2cErrorKey(e))); });
      else void getC2COrders({ status: "active" }).then(page => { if (alive.current) setActiveCount(page.items.length); }).catch(() => {});
    };
    const timer = setInterval(refresh, 15_000); document.addEventListener("visibilitychange", refresh);
    return () => { clearInterval(timer); document.removeEventListener("visibilitychange", refresh); refreshVersion.current++; };
  }, [load, history, t]);
  React.useEffect(() => {
    const refresh = () => {
      if (document.visibilityState === "visible" && !pauseLock.current) void loadSettings().catch(e => { if (alive.current) setSellerError(t(c2cErrorKey(e))); });
    };
    refresh(); const timer = setInterval(refresh, 30_000);
    document.addEventListener("visibilitychange", refresh);
    const off = subscribeMarketBalanceChanged(actor, refresh);
    return () => { clearInterval(timer); document.removeEventListener("visibilitychange", refresh); off(); settingsVersion.current++; };
  }, [actor, loadSettings, t]);
  React.useEffect(() => { if (selectedId) void load().catch(() => {}); }, [selectedId, load]);
  const reload = () => {
    setBusy(true);
    void Promise.all([
      load().catch(e => { if (alive.current) setError(t(c2cErrorKey(e))); }),
      loadSettings().catch(e => { if (alive.current) setSellerError(t(c2cErrorKey(e))); }),
    ]).finally(() => { if (alive.current) setBusy(false); });
  };
  const pause = async (resume = false) => {
    if (!snapshot?.seller || pauseLock.current) return;
    pauseLock.current = true; settingsVersion.current++; setPausing(true); setSellerError(""); setNotice("");
    try {
      const seller = await (resume ? resumeC2CSeller(snapshot.seller) : pauseC2CSeller(snapshot.seller));
      if (alive.current && identity.current === actor) { setSnapshot(old => old ? { ...old, seller, state: resume ? "selling" : "paused" } : old); setNotice(t(resume ? "c2c.accepting" : "c2c.paused")); await loadSettings(); }
    } catch (cause) {
      if (alive.current) setSellerError(t(c2cErrorKey(cause)));
    } finally { pauseLock.current = false; if (alive.current) setPausing(false); }
  };
  const saved = (seller: C2CSeller) => {
    settingsVersion.current++;
    setSnapshot(old => old ? { ...old, seller } : old);
    setEditorBusy(false); setSellOpen(false); setNotice(t("c2c.saved")); reload();
  };
  const openSellerSettings = () => { setNotice(""); setEditorKey(v => v + 1); setSellOpen(true); };
  const sellerSettingsLabel = t(snapshot?.state === "confirmation_required" ? "c2c.confirmAutoAccept" : snapshot?.seller ? "c2c.sellerSettings" : "c2c.seller");
  return <section id="c2c-orders" className={embedded ? "grid min-w-0 gap-4" : "grid min-w-0 gap-4 rounded-xl border border-border bg-card p-4 sm:p-6"} data-c2c-orders>
    {embedded ? null : <header className="flex min-w-0 flex-wrap items-center justify-between gap-3">
      <h2 className="text-lg font-semibold">{t("c2c.title")}</h2>
      <C2CAction type="button" secondary disabled={!policy || !snapshot || pausing} onClick={openSellerSettings}>{sellerSettingsLabel}</C2CAction>
    </header>}
    {snapshot?.seller ? <div className="flex min-w-0 flex-wrap items-center justify-between gap-2 text-sm">
      <p className="min-w-0 break-words text-muted-foreground">
        {t(`c2c.sellerState.${snapshot.state}`)}
        {" · "}{t("c2c.available", { amount: formatWalletUnits(snapshot.availableUnits) })}
        {snapshot.pendingOrders ? ` · ${t("c2c.pendingCount", { count: snapshot.pendingOrders })}` : ""}
      </p>
      <div className="flex flex-wrap gap-2">{embedded && sellerSettingsLabel !== t("c2c.seller") ? <C2CAction type="button" secondary disabled={!policy || !snapshot || pausing} onClick={openSellerSettings}>{sellerSettingsLabel}</C2CAction> : null}{snapshot.seller.autoPauseReason && snapshot.seller.autoAcceptTermsVersion === 3 && snapshot.seller.enabled && !snapshot.seller.suspended ? <C2CAction type="button" phase={pausing ? "loading" : "default"} disabled={pausing || !snapshot.canEnable} onClick={() => void pause(true)}>{t("c2c.resumeSelling")}</C2CAction> : null}
      {snapshot.seller.enabled ? <C2CAction type="button" secondary phase={pausing ? "loading" : "default"} disabled={pausing} onClick={() => void pause()}>{t("c2c.pauseSelling")}</C2CAction> : null}</div>
    </div> : null}
    {snapshot?.state === "confirmation_required" ? <p role="status" className="text-sm text-muted-foreground">{t("c2c.confirmAutoAcceptHelp")}</p> : null}
    {snapshot?.seller?.autoPauseReason ? <p role="status" className="text-sm text-muted-foreground">{t(`c2c.pauseReason.${snapshot.seller.autoPauseReason}`)}</p> : null}
    <C2CError>{sellerError}</C2CError>
    {notice ? <p role="status" className="text-sm text-primary">{notice}</p> : null}
    <div className="flex min-w-0 flex-wrap items-center justify-between gap-2">
      <div role="group" aria-label={t("c2c.orders")} className="flex flex-wrap gap-1">
        <C2CAction type="button" secondary={history} aria-pressed={!history} onClick={() => setHistory(false)}>{t("c2c.activeOrders")}{activeCount ? ` (${activeCount})` : ""}</C2CAction>
        <C2CAction type="button" secondary={!history} aria-pressed={history} onClick={() => setHistory(true)}>{t("c2c.history")}</C2CAction>
      </div>
      <C2CAction type="button" secondary disabled={busy || pausing} onClick={reload} aria-label={t("common.reload")}><RefreshCw className="size-4" aria-hidden="true" /></C2CAction>
    </div>
    <C2CError>{error}</C2CError>
    {!loaded && !error ? <p role="status" className="text-sm text-muted-foreground">{t("common.loading")}</p> : !items.length ? <p className="py-3 text-sm text-muted-foreground">{t("c2c.noOrders")}</p> : <ul className="grid max-h-96 min-w-0 gap-2 overflow-y-auto overscroll-contain p-1">
      {items.map(o => <li key={o.id}><button type="button" onClick={() => onSelect(o.id)} className="flex min-h-11 w-full min-w-0 flex-wrap items-center justify-between gap-3 rounded-md px-3 py-3 text-left hover:bg-muted focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring active:translate-y-px motion-reduce:transform-none">
        <span className="grid min-w-0 gap-1"><span className="text-base font-semibold tabular-nums">{t(o.buyerId === actor ? "c2c.buying" : "c2c.selling")} · {formatWalletUnits(o.amountUnits)}</span>
          <span className="break-words text-xs text-muted-foreground">{o.sellerName} · {c2cPayLabel(o.payMinor, o.currency)}</span></span>
        <span className="flex flex-wrap items-center gap-2"><C2CStatusLabel status={o.status} /><ChevronRight className="size-4 text-muted-foreground" aria-hidden="true" /></span>
      </button></li>)}
    </ul>}
    {next ? <C2CAction type="button" secondary disabled={busy} onClick={() => { setBusy(true); void load(next).catch(e => { if (alive.current) setError(t(c2cErrorKey(e))); }).finally(() => { if (alive.current) setBusy(false); }); }}>{t("common.next")}</C2CAction> : null}
    <C2CDialog open={selling && !!policy && !!snapshot} title={t(snapshot?.seller ? "c2c.sellerSettings" : "c2c.seller")} busy={editorBusy} onClose={() => setSellOpen(false)}>
      {selling && policy && snapshot ? <C2CSellerPanel key={editorKey} policy={policy} snapshot={snapshot} availabilityError={!!sellerError} onRefresh={async () => { await loadSettings(); }} onBusyChange={setEditorBusy} onSaved={saved}
        onReload={async () => { if (!await loadSettings()) throw new Error("Seller settings reload superseded"); if (alive.current) { setEditorBusy(false); setEditorKey(v => v + 1); } }} /> : null}
    </C2CDialog>
    {selectedId ? <C2COrderDialog key={selectedId} actor={actor} id={selectedId} onClose={() => { onSelect(null); reload(); }} onChanged={() => { notifyMarketBalanceChanged(actor); reload(); }} /> : null}
  </section>;
}

function C2COrderDialog({ actor, id, onClose, onChanged }: { actor: string; id: string; onClose: () => void; onChanged: () => void }) {
  const { t, locale } = useLocaleText(); const [o, setOrder] = React.useState<C2COrder | null>(null); const [events, setEvents] = React.useState<C2CEvent[]>([]); const [next, setNext] = React.useState<string | null>(null);
  const [error, setError] = React.useState(""); const [busy, setBusy] = React.useState(false); const [intent, setIntent] = React.useState<"paid" | "cancel" | null>(null);
  const [ack, setAck] = React.useState(false); const [note, setNote] = React.useState(""); const [releaseAck, setReleaseAck] = React.useState(false);
  const [now, setNow] = React.useState(c2cNow);
  const alive = React.useRef(true); const lock = React.useRef(false); const request = React.useRef(0); const seen = React.useRef(0); const eventsExpanded = React.useRef(false);
  React.useEffect(() => { alive.current = true; return () => { alive.current = false; request.current++; }; }, []);
  const load = React.useCallback(async () => {
    const seq = ++request.current; const [order, page] = await Promise.all([getC2COrder(id), getC2CEvents(id)]);
    if (!alive.current || seq !== request.current) return;
    if (order.buyerId !== actor && order.sellerId !== actor) throw new Error("C2C ownership changed");
    if (seen.current && seen.current !== order.version) { setReleaseAck(false); notifyMarketBalanceChanged(actor); }
    seen.current = order.version; setOrder(order); setNow(c2cNow());
    setEvents(old => eventsExpanded.current ? [...page.items, ...old.filter(e => !page.items.some(n => n.id === e.id))] : page.items);
    if (!eventsExpanded.current) setNext(page.next);
  }, [actor, id]);
  React.useEffect(() => {
    void load().catch(e => { if (alive.current) setError(t(c2cErrorKey(e))); });
    const tick = setInterval(() => { setNow(c2cNow()); }, 1000);
    const refresh = () => { if (document.visibilityState === "visible" && !lock.current) void load().catch(e => { if (alive.current) setError(t(c2cErrorKey(e))); }); };
    const poll = setInterval(refresh, 10_000); document.addEventListener("visibilitychange", refresh);
    return () => { clearInterval(tick); clearInterval(poll); document.removeEventListener("visibilitychange", refresh); };
  }, [load, t]);
  const run = async (fn: () => Promise<C2COrder | void>) => {
    if (lock.current) return; lock.current = true; request.current++; setBusy(true); setError("");
    try { const result = await fn(); if (!alive.current) return; if (result) { setOrder(result); seen.current = result.version; setIntent(null); setAck(false); setNote(""); setReleaseAck(false); onChanged(); } await load(); }
    catch (e) { if (alive.current) { setError(t(c2cErrorKey(e))); void load().catch(() => {}); } }
    finally { lock.current = false; if (alive.current) setBusy(false); }
  };
  const buyer = o?.buyerId === actor; const window = o ? c2cPaymentWindow(o, now) : { mayStartPayment: false, mayClaimPayment: false };
  const held = o?.status === "paid" || o?.status === "disputed"; const closed = o?.status === "cancelled" || o?.status === "expired";
  return <C2CDialog open title={t("c2c.detail")} busy={busy} onClose={onClose}>
    <C2CError>{error}</C2CError>
    {!o ? <C2CAction type="button" secondary onClick={() => void run(load)}>{t("common.reload")}</C2CAction> : <>
      <div className="flex flex-wrap items-center justify-between gap-3"><p className="text-2xl font-semibold tabular-nums">{formatWalletUnits(o.amountUnits)}</p><C2CStatusLabel status={o.status} /></div>
      <p className="break-all font-mono text-xs text-muted-foreground">{t("c2c.id")}: {o.id}</p>
      <dl className="grid min-w-0 grid-cols-[minmax(0,1fr)_auto] gap-2 text-sm"><dt className="min-w-0 [overflow-wrap:anywhere]">{o.sellerName}</dt><dd className="font-semibold tabular-nums">{c2cPayLabel(o.payMinor, o.currency)}</dd></dl>
      {o.status === "awaiting_seller" ? <div role="status" className="grid gap-2 rounded-md bg-muted p-3 text-sm"><p className="font-semibold tabular-nums">{now < Date.parse(o.acceptanceDeadline!) ? t("c2c.acceptanceCountdown", { time: formatTopupCountdown(Math.max(0, Math.ceil((Date.parse(o.acceptanceDeadline!) - now) / 1000))) }) : t("c2c.acceptanceElapsed")}</p><p className="text-muted-foreground">{t(buyer ? "c2c.waitSeller" : "c2c.sellerApplication")}</p></div> : null}
      {o.status === "awaiting_payment" ? <div role="status" className="grid gap-2 rounded-md bg-muted p-3 text-sm"><p className="font-semibold tabular-nums">{window.mayStartPayment ? t("c2c.deadline", { time: formatTopupCountdown(Math.max(0, Math.ceil((Date.parse(o.paymentDeadline!) - now) / 1000))) }) : t("c2c.deadlinePassed")}</p><p className="text-xs leading-5 text-muted-foreground">{t("c2c.deadlineHelp")}</p></div> : null}
      {(window.mayStartPayment || held || o.status === "released" || (closed && o.latePaymentReported && !o.latePaymentResolved)) && (o.description || o.legacyPayment) ? <div className="grid min-w-0 gap-2 text-sm">
        <h3 className="font-medium">{t("c2c.description")}</h3>
        {o.description ? <C2CDescription value={o.description} links={window.mayStartPayment || held} /> : o.legacyPayment ? <C2CDescription value={[o.legacyPayment.paymentMethod, o.legacyPayment.contact, o.legacyPayment.paymentDetails].join("\n")} links={window.mayStartPayment || held} /> : null}
        {(window.mayStartPayment || held || (closed && o.latePaymentReported && !o.latePaymentResolved)) ? <p className="text-xs text-muted-foreground">{t(closed ? "c2c.lateHint" : held ? "c2c.paidHint" : "c2c.frozen")}</p> : null}
      </div> : null}
      {closed ? <p role="status" className="text-sm text-muted-foreground">{t("c2c.closedNoPayment")}</p> : null}
      {o.status === "awaiting_seller" && !buyer ? <div className="flex flex-wrap gap-2"><C2CAction type="button" disabled={busy || now >= Date.parse(o.acceptanceDeadline!)} phase={busy ? "loading" : "default"} onClick={() => void run(() => actC2C(o, "accept"))}>{t("c2c.acceptApplication")}</C2CAction><C2CAction type="button" secondary disabled={busy} onClick={() => void run(() => actC2C(o, "reject"))}>{t("c2c.rejectApplication")}</C2CAction></div> : null}
      {o.status === "awaiting_seller" && buyer ? <div className="grid gap-2"><C2CCheck checked={ack} disabled={busy} onChange={e => setAck(e.target.checked)}>{t("c2c.cancelAck")}</C2CCheck><C2CAction type="button" secondary disabled={busy || !ack} phase={busy ? "loading" : "default"} onClick={() => void run(() => actC2C(o, "cancel"))}>{t("c2c.withdrawApplication")}</C2CAction></div> : null}
      {buyer && o.status === "awaiting_payment" ? intent ? <div className="grid gap-2"><C2CCheck checked={ack} disabled={busy} onChange={e => setAck(e.target.checked)}>{t(intent === "paid" ? "c2c.paidAck" : "c2c.cancelAck")}</C2CCheck><div className="flex flex-wrap gap-2"><C2CAction type="button" disabled={!ack || busy || (intent === "paid" && !window.mayClaimPayment)} phase={busy ? "loading" : "default"} onClick={() => void run(() => actC2C(o, intent))}>{t(intent === "paid" ? "c2c.markPaid" : "c2c.cancel")}</C2CAction><C2CAction type="button" secondary disabled={busy} onClick={() => setIntent(null)}>{t("common.cancel")}</C2CAction></div></div> : <div className="flex flex-wrap gap-2"><C2CAction type="button" disabled={busy || !window.mayClaimPayment} onClick={() => { setIntent("paid"); setAck(false); }}>{t("c2c.markPaid")}</C2CAction><C2CAction type="button" secondary disabled={busy} onClick={() => { setIntent("cancel"); setAck(false); }}>{t("c2c.cancel")}</C2CAction></div> : null}
      {!buyer && held && (o.flowVersion !== 2 || o.acceptedAt !== null) ? <div className="grid gap-3">
        <C2CCheck checked={releaseAck} disabled={busy} onChange={e => setReleaseAck(e.target.checked)}>{t("c2c.releaseAck")}</C2CCheck>
        <C2CAction type="button" disabled={!releaseAck || busy} phase={busy ? "loading" : "default"} onClick={() => void run(() => releaseC2C(o))}>{t("c2c.release")}</C2CAction>
      </div> : null}
      {o.status === "awaiting_seller" && buyer ? <details><summary className="min-h-11 cursor-pointer py-3 text-sm focus-visible:outline-2 focus-visible:outline-ring">{t("c2c.earlyPayment")}</summary><div className="grid gap-2"><p className="text-sm text-muted-foreground">{t("c2c.earlyPaymentHelp")}</p><label className="grid gap-1 text-sm">{t("c2c.note")}<textarea className={C2C_INPUT} rows={3} maxLength={1500} value={note} disabled={busy} onChange={e => setNote(e.target.value)} /></label><C2CAction type="button" secondary disabled={busy || !note.trim()} onClick={() => void run(() => actC2C(o, "late-payment", note))}>{t("c2c.reportEarlyPayment")}</C2CAction></div></details> : null}
      {(held || (closed && (buyer || o.latePaymentReported))) ? <div className="grid gap-2">
        {closed ? <p className="text-xs leading-5 text-muted-foreground">{t("c2c.lateHint")}</p> : null}
        <label className="grid gap-1 text-sm">{t("c2c.note")}<textarea className={C2C_INPUT} rows={3} value={note} maxLength={1500} disabled={busy} onChange={e => setNote(e.target.value)} /></label>
        <div className="flex flex-wrap gap-2">{o.status === "paid" ? <C2CAction type="button" secondary disabled={!note.trim() || busy} onClick={() => void run(() => actC2C(o, "dispute", note))}>{t("c2c.dispute")}</C2CAction> : null}
          {buyer && closed && !o.latePaymentReported ? <C2CAction type="button" secondary disabled={!note.trim() || busy} onClick={() => void run(() => actC2C(o, "late-payment", note))}>{t("c2c.late")}</C2CAction> : null}
          {held || o.latePaymentReported ? <C2CAction type="button" secondary disabled={!note.trim() || busy} onClick={() => void run(() => actC2C(o, "notes", note))}>{t("c2c.addNote")}</C2CAction> : null}
        </div>
      </div> : null}
      {o.status === "released" ? <p role="status" className="text-sm text-primary">{t("c2c.status.released")}{o.releasedAt ? ` · ${new Date(o.releasedAt).toLocaleString(locale)}` : ""}</p> : null}
      <details className="min-w-0"><summary className="min-h-11 cursor-pointer py-3 text-sm font-medium focus-visible:outline-2 focus-visible:outline-ring">{t("c2c.events")}</summary><div className="grid max-h-64 gap-3 overflow-y-auto overscroll-contain"><C2CEvents items={events} />{next ? <C2CAction type="button" secondary disabled={busy} onClick={() => void run(async () => { const p = await getC2CEvents(o.id, next); if (alive.current) { eventsExpanded.current = true; setEvents(old => [...old, ...p.items.filter(e => !old.some(v => v.id === e.id))]); setNext(p.next); } })}>{t("common.next")}</C2CAction> : null}</div></details>
      <C2CAction type="button" secondary disabled={busy} onClick={() => void run(load)}>{t("common.reload")}</C2CAction>
    </>}
  </C2CDialog>;
}
