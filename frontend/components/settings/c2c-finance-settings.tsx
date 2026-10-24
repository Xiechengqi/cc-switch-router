"use client";
/* Hallmark · component: C2C finance controls · genre: modern-minimal · theme: existing Router tokens
 * states: default · hover · focus · active · disabled · loading · error · success
 * pre-emit critique: P5 H4 E4 S5 R5 V4
 */
import * as React from "react";
import { useAuth } from "@/components/auth/auth-provider";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { C2CEvents } from "@/components/dashboard/c2c-orders-panel";
import { C2CDescription } from "@/components/market/c2c-description";
import { C2CAction, C2CCheck, C2CDialog, C2CError, C2CStatusLabel, C2C_INPUT } from "@/components/market/c2c-primitives";
import { c2cErrorKey, c2cPayLabel, getAdminC2COrder, getAdminC2CPolicy, getAdminC2CSellers, getC2CEvents, getC2COrders, parseC2CMinor, resolveC2C, saveAdminC2CPolicy, suspendC2CSeller, getC2CDisputeHandlers, saveC2CDisputeHandler, type C2CDisputeHandler, type C2CAdminSeller, type C2COrder, type C2CPolicy } from "@/lib/market-c2c";
import { formatWalletUnits } from "@/lib/market-wallet-money";

export function C2CFinanceSettings() {
  const { session } = useAuth();
  return session?.authenticated && session.user?.id ? <C2CFinanceControl key={session.user.id} actor={session.user.id} /> : null;
}
function C2CFinanceControl({ actor }: { actor: string }) {
  const { t } = useLocaleText(); const [policy, setPolicy] = React.useState<C2CPolicy | null>(null); const [canManage, setCanManage] = React.useState(false); const [emailReady, setEmailReady] = React.useState(true);
  const [enabled, setEnabled] = React.useState(true); const [admission, setAdmission] = React.useState("100"); const [min, setMin] = React.useState("1"); const [max, setMax] = React.useState("5000"); const [minutes, setMinutes] = React.useState("30");
  const [acceptance, setAcceptance] = React.useState("10");
  const [handlers, setHandlers] = React.useState<C2CDisputeHandler[]>([]); const [handlerNext, setHandlerNext] = React.useState<string | null>(null); const [handlerEmail, setHandlerEmail] = React.useState("");
  const [orders, setOrders] = React.useState<C2COrder[]>([]); const [next, setNext] = React.useState<string | null>(null); const [all, setAll] = React.useState(false);
  const [sellers, setSellers] = React.useState<C2CAdminSeller[]>([]); const [sellerNext, setSellerNext] = React.useState<string | null>(null);
  const [selected, setSelected] = React.useState<string | null>(null); const [control, setControl] = React.useState<C2CAdminSeller | null>(null); const [reason, setReason] = React.useState("");
  const [error, setError] = React.useState(""); const [busy, setBusy] = React.useState(false); const [saved, setSaved] = React.useState(false); const lock = React.useRef(false); const alive = React.useRef(true);
  const orderRequest = React.useRef(0); const sellerRequest = React.useRef(0);
  React.useEffect(() => { alive.current = true; return () => { alive.current = false; orderRequest.current++; sellerRequest.current++; }; }, []);
  React.useEffect(() => { const id = new URLSearchParams(window.location.search).get("c2cOrder"); if (id && /^[A-Za-z0-9_-]{1,128}$/.test(id)) setSelected(id); }, []);
  const loadPolicy = React.useCallback(async () => {
    const p = await getAdminC2CPolicy(); if (!alive.current) return;
    setPolicy(p.policy); setCanManage(p.canManage); setEmailReady(p.emailReady); setEnabled(p.policy.enabled); setAdmission(String(p.policy.admissionMinor / 100)); setMin(String(p.policy.minMinor / 100)); setMax(String(p.policy.maxMinor / 100)); setMinutes(String(p.policy.paymentMinutes));
    setAcceptance(String(p.policy.acceptanceMinutes));
  }, []);
  const loadOrders = React.useCallback(async (before = "") => {
    const seq = ++orderRequest.current;
    const p = await getC2COrders({ status: all ? "" : "exceptions", before }, true); if (!alive.current || seq !== orderRequest.current) return;
    setOrders(old => before ? [...old, ...p.items.filter(o => !old.some(v => v.id === o.id))] : p.items); setNext(p.next);
  }, [all]);
  const loadSellers = React.useCallback(async (before = "") => {
    const seq = ++sellerRequest.current;
    const p = await getAdminC2CSellers(before); if (!alive.current || seq !== sellerRequest.current) return;
    setSellers(old => before ? [...old, ...p.items.filter(s => !old.some(v => v.userId === s.userId))] : p.items); setSellerNext(p.next);
  }, []);
  const run = React.useCallback(async (fn: () => Promise<unknown>) => {
    if (lock.current) return; lock.current = true; setBusy(true); setError("");
    try { await fn(); } catch (e) { if (alive.current) setError(t(c2cErrorKey(e))); }
    finally { lock.current = false; if (alive.current) setBusy(false); }
  }, [t]);
  const loadHandlers = async (before = "") => { const p = await getC2CDisputeHandlers(before); if (alive.current) { setHandlers(old => before ? [...old, ...p.items.filter(h => !old.some(v => v.userId === h.userId))] : p.items); setHandlerNext(p.next); } };
  React.useEffect(() => { void run(async () => { await Promise.all([loadPolicy(), loadSellers()]); }); }, [loadPolicy, loadSellers, run]);
  React.useEffect(() => { setOrders([]); setNext(null); void loadOrders().catch(e => { if (alive.current) setError(t(c2cErrorKey(e))); }); return () => { orderRequest.current++; }; }, [loadOrders, t]);
  const low = parseC2CMinor(min); const high = parseC2CMinor(max); const threshold = parseC2CMinor(admission); const m = /^\d{1,2}$/.test(minutes) ? Number(minutes) : 0;
  const a = /^\d{1,2}$/.test(acceptance) ? Number(acceptance) : 0;
  const valid = low != null && high != null && high >= low && high <= 10_000_000 && threshold != null && threshold >= 100 && threshold <= 10_000_000 && m >= 5 && m <= 60 && a >= 5 && a <= 30;
  return <section className="grid min-w-0 gap-5 rounded-xl border border-border bg-card p-4 sm:p-5" data-c2c-admin>
    <header className="flex flex-wrap items-center justify-between gap-3"><h3 className="text-base font-semibold">{t("c2c.policy")}</h3><C2CAction type="button" secondary disabled={busy} onClick={() => void run(async () => { setSaved(false); await Promise.all([loadPolicy(), loadOrders(), loadSellers()]); })}>{t("common.reload")}</C2CAction></header>
    <C2CError>{error}</C2CError>
    {!emailReady ? <p className="text-sm text-muted-foreground">{t("c2c.error.email")}</p> : null}
    {policy ? <form className="grid min-w-0 gap-3" onSubmit={e => { e.preventDefault(); if (!valid || !canManage || threshold == null || low == null || high == null) return; void run(async () => { const p = await saveAdminC2CPolicy({ ...policy, enabled, admissionMinor: threshold, minMinor: low, maxMinor: high, paymentMinutes: m, acceptanceMinutes: a }); if (alive.current) { setPolicy(p); setSaved(true); } }); }}>
      <p className="text-sm leading-6 text-muted-foreground">{t("c2c.policyHelp")}</p>
      <C2CCheck checked={enabled} disabled={!canManage || busy} onChange={e => { setEnabled(e.target.checked); setSaved(false); }}>{t("c2c.enabled")}</C2CCheck>
      <div className="grid min-w-0 gap-3 sm:grid-cols-2">
        <label className="grid min-w-0 gap-1 text-sm">{t("c2c.admission")}<input className={C2C_INPUT} inputMode="decimal" value={admission} disabled={!canManage || busy} onChange={e => { setAdmission(e.target.value); setSaved(false); }} /></label>
        <label className="grid min-w-0 gap-1 text-sm">{t("c2c.minutes")}<input className={C2C_INPUT} inputMode="numeric" value={minutes} disabled={!canManage || busy} onChange={e => { setMinutes(e.target.value); setSaved(false); }} /></label>
        <label className="grid min-w-0 gap-1 text-sm">{t("c2c.acceptanceMinutes")}<input className={C2C_INPUT} inputMode="numeric" value={acceptance} disabled={!canManage || busy} onChange={e => { setAcceptance(e.target.value); setSaved(false); }} /></label>
        <label className="grid min-w-0 gap-1 text-sm">{t("c2c.min")}<input className={C2C_INPUT} inputMode="decimal" value={min} disabled={!canManage || busy} onChange={e => { setMin(e.target.value); setSaved(false); }} /></label>
        <label className="grid min-w-0 gap-1 text-sm">{t("c2c.max")}<input className={C2C_INPUT} inputMode="decimal" value={max} disabled={!canManage || busy} onChange={e => { setMax(e.target.value); setSaved(false); }} /></label>
      </div>
      <div className="flex flex-wrap items-center gap-3"><C2CAction type="submit" phase={busy ? "loading" : saved ? "success" : "default"} disabled={!valid || !canManage || busy}>{t("common.save")}</C2CAction>{saved ? <span role="status" className="text-sm text-muted-foreground">{t("common.save")} ✓</span> : null}{!canManage ? <p className="text-xs text-muted-foreground">{t("walletPolicy.readOnly")}</p> : null}</div>
    </form> : <p role="status" className="text-sm text-muted-foreground">{t("common.loading")}</p>}
    <details><summary className="min-h-11 cursor-pointer py-3 text-sm font-semibold focus-visible:outline-2 focus-visible:outline-ring">{t("c2c.exceptions")}{orders.length ? ` · ${orders.length}${next ? "+" : ""}` : ""}</summary>
      <div className="grid min-w-0 gap-3"><C2CAction type="button" secondary disabled={busy} aria-pressed={all} onClick={() => setAll(v => !v)}>{t(all ? "c2c.exceptions" : "c2c.allOrders")}</C2CAction>
        {orders.length ? <ul className="grid max-h-80 gap-2 overflow-y-auto overscroll-contain p-1">{orders.map(o => <li key={o.id}><button type="button" onClick={() => setSelected(o.id)} className="flex min-h-11 w-full min-w-0 flex-wrap items-center justify-between gap-2 rounded-md bg-muted/40 p-3 text-left hover:bg-muted focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring active:translate-y-px motion-reduce:transform-none"><span className="grid min-w-0 gap-1"><span className="text-sm tabular-nums">{formatWalletUnits(o.amountUnits)} · {o.sellerName}</span><span className="break-all font-mono text-xs text-muted-foreground">{o.id}</span></span><C2CStatusLabel status={o.status} /></button></li>)}</ul> : <p className="text-sm text-muted-foreground">{t("c2c.noExceptions")}</p>}
        {next ? <C2CAction type="button" secondary disabled={busy} onClick={() => void run(() => loadOrders(next))}>{t("common.next")}</C2CAction> : null}
      </div>
    </details>
    <details><summary className="min-h-11 cursor-pointer py-3 text-sm font-semibold focus-visible:outline-2 focus-visible:outline-ring">{t("c2c.adminSellers")}</summary><div className="grid gap-3"><ul className="grid max-h-64 gap-2 overflow-y-auto overscroll-contain p-1">{sellers.map(s => <li key={s.userId} className="flex min-w-0 flex-wrap items-center justify-between gap-2 text-sm"><span className="min-w-0 break-words">{s.displayName}{s.suspended ? ` · ${t("c2c.suspended")}` : ""}</span><C2CAction type="button" secondary disabled={!canManage || busy} onClick={() => { setControl(s); setReason(""); }}>{t(s.suspended ? "c2c.unsuspend" : "c2c.suspend")}</C2CAction></li>)}</ul>{sellerNext ? <C2CAction type="button" secondary disabled={busy} onClick={() => void run(() => loadSellers(sellerNext))}>{t("common.next")}</C2CAction> : null}</div></details>
    <C2CDialog open={!!control} title={t(control?.suspended ? "c2c.unsuspend" : "c2c.suspend")} busy={busy} onClose={() => setControl(null)}>
      <label className="grid gap-1 text-sm">{t("c2c.reason")}<textarea className={C2C_INPUT} value={reason} maxLength={500} rows={3} disabled={busy} onChange={e => setReason(e.target.value)} /></label><C2CError>{error}</C2CError>
      <C2CAction type="button" disabled={!canManage || !reason.trim() || busy} phase={busy ? "loading" : "default"} onClick={() => { if (control) void run(async () => { await suspendC2CSeller(control, reason); if (alive.current) setControl(null); await loadSellers(); }); }}>{t("common.save")}</C2CAction>
    </C2CDialog>
    {canManage ? <details onToggle={e => { if (e.currentTarget.open) void run(() => loadHandlers()); }}><summary className="min-h-11 cursor-pointer py-3 text-sm font-semibold focus-visible:outline-2 focus-visible:outline-ring">{t("c2c.handlers")}</summary><div className="grid min-w-0 gap-3">
      <p className="text-sm text-muted-foreground">{t("c2c.handlersHelp")}</p>
      <form className="grid min-w-0 gap-2" onSubmit={e => { e.preventDefault(); const email = handlerEmail.trim().toLowerCase(); if (!email || busy) return; void run(async () => { await saveC2CDisputeHandler(email, handlers.find(h => h.email === email)?.version ?? 0, true); if (alive.current) setHandlerEmail(""); await loadHandlers(); }); }}><label className="grid min-w-0 gap-1 text-sm">{t("c2c.handlerEmail")}<input className={C2C_INPUT} type="email" required maxLength={320} value={handlerEmail} disabled={busy} onChange={e => setHandlerEmail(e.target.value)} /></label><C2CAction type="submit" disabled={busy || !handlerEmail.trim()} phase={busy ? "loading" : "default"}>{t("c2c.grantHandler")}</C2CAction></form>
      <ul className="grid max-h-64 min-w-0 gap-3 overflow-y-auto overscroll-contain">{handlers.map(h => <li key={h.userId} className="flex min-w-0 flex-wrap items-center justify-between gap-2 text-sm"><span className="min-w-0 break-all">{h.email}{!h.enabled ? ` · ${t("c2c.handlerDisabled")}` : !h.isAdmin ? ` · ${t("c2c.handlerInactive")}` : ""}</span>{h.enabled ? <C2CAction type="button" secondary disabled={busy} onClick={() => void run(async () => { await saveC2CDisputeHandler(h.email, h.version, false); await loadHandlers(); })}>{t("c2c.revokeHandler")}</C2CAction> : null}</li>)}</ul>
      {handlerNext ? <C2CAction type="button" secondary disabled={busy} onClick={() => void run(() => loadHandlers(handlerNext))}>{t("common.next")}</C2CAction> : null}
    </div></details> : null}
    {selected ? <C2CResolutionDialog key={selected} id={selected} actor={actor} onClose={() => setSelected(null)} onResolved={() => { void loadOrders().catch(e => setError(t(c2cErrorKey(e)))); }} /> : null}
  </section>;
}
function C2CResolutionDialog({ id, actor, onClose, onResolved }: { id: string; actor: string; onClose: () => void; onResolved: () => void }) {
  const { t } = useLocaleText(); const [detail, setDetail] = React.useState<Awaited<ReturnType<typeof getAdminC2COrder>> | null>(null);
  const [evidence, setEvidence] = React.useState(""); const [action, setAction] = React.useState(""); const [ack, setAck] = React.useState(false); const [busy, setBusy] = React.useState(false); const [error, setError] = React.useState(""); const lock = React.useRef(false); const alive = React.useRef(true);
  const load = React.useCallback(async () => { const d = await getAdminC2COrder(id); if (alive.current) { setDetail(d); setAck(false); setAction(""); } }, [id]);
  React.useEffect(() => { alive.current = true; void load().catch(e => { if (alive.current) setError(t(c2cErrorKey(e))); }); return () => { alive.current = false; }; }, [load, t]);
  const o = detail?.order; const conflict = o?.buyerId === actor || o?.sellerId === actor;
  const canManage = detail?.canResolve === true;
  const mayResolve = !!o && (o.status === "disputed" || (o.latePaymentReported && !o.latePaymentResolved));
  return <C2CDialog open title={t("c2c.resolution")} busy={busy} onClose={onClose}>
    <C2CError>{error}</C2CError>
    {o ? <>
      <div className="flex flex-wrap items-center justify-between gap-2"><span className="text-xl font-semibold tabular-nums">{formatWalletUnits(o.amountUnits)}</span><C2CStatusLabel status={o.status} /></div><p className="break-all font-mono text-xs text-muted-foreground">{o.id}</p>
      <p className="text-sm">{o.sellerName} · {c2cPayLabel(o.payMinor, o.currency)}</p>
      {o.description ? <C2CDescription value={o.description} links={false} /> : o.legacyPayment ? <C2CDescription value={[o.legacyPayment.paymentMethod, o.legacyPayment.contact, o.legacyPayment.paymentDetails].join("\n")} links={false} /> : null}
      {!detail?.evidenceValid ? <C2CError>{t("c2c.evidenceInvalid")}</C2CError> : null}
      {conflict || !canManage ? <p role="status" className="text-sm text-muted-foreground">{t(conflict ? "c2c.conflict" : "c2c.resolveReadOnly")}</p> : null}
      <C2CEvents items={detail?.events.items ?? []} />
      {detail?.events.next ? <C2CAction type="button" secondary disabled={busy} onClick={() => {
        if (lock.current) return; lock.current = true; setBusy(true); setError("");
        void getC2CEvents(id, detail.events.next!, true).then(page => {
          if (alive.current) setDetail(current => current ? { ...current, events: { items: [...current.events.items, ...page.items.filter(e => !current.events.items.some(v => v.id === e.id))], next: page.next } } : current);
        }).catch(e => { if (alive.current) setError(t(c2cErrorKey(e))); }).finally(() => { lock.current = false; if (alive.current) setBusy(false); });
      }}>{t("common.next")}</C2CAction> : null}
      {mayResolve ? <>
        <select aria-label={t("c2c.resolution")} className={C2C_INPUT} value={action} disabled={!canManage || conflict || busy || !detail?.evidenceValid} onChange={e => { setAction(e.target.value); setAck(false); }}><option value="">—</option>{o.status === "disputed" ? <><option value="release">{t("c2c.resolveRelease")}</option><option value="return_to_seller">{t("c2c.resolveReturn")}</option></> : <option value="record_external_resolution">{t("c2c.resolveExternal")}</option>}</select>
        <label className="grid gap-1 text-sm">{t("c2c.note")}<textarea className={C2C_INPUT} rows={4} value={evidence} maxLength={1500} disabled={!canManage || conflict || busy} onChange={e => setEvidence(e.target.value)} /></label>
        <C2CCheck checked={ack} disabled={!canManage || conflict || busy} onChange={e => setAck(e.target.checked)}>{t("c2c.resolveAck")}</C2CCheck>
        <C2CAction type="button" disabled={!canManage || conflict || busy || !ack || !action || !evidence.trim() || !detail?.evidenceValid} phase={busy ? "loading" : "default"} onClick={() => {
          if (lock.current) return; lock.current = true; setBusy(true); setError("");
          void resolveC2C(o, action, evidence).then(async () => { if (alive.current) { onResolved(); await load(); } }).catch(e => { if (alive.current) setError(t(c2cErrorKey(e))); }).finally(() => { lock.current = false; if (alive.current) setBusy(false); });
        }}>{t("common.save")}</C2CAction>
      </> : null}
    </> : <p role="status">{t("common.loading")}</p>}
    <C2CAction type="button" secondary disabled={busy} onClick={() => { setError(""); void load().catch(e => setError(t(c2cErrorKey(e)))); }}>{t("common.reload")}</C2CAction>
  </C2CDialog>;
}
