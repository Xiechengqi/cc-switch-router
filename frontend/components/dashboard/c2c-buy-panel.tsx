"use client";
/* Hallmark · component: C2C purchase · genre: modern-minimal · theme: existing Router tokens
 * states: default · hover · focus · active · disabled · loading · error · success
 * pre-emit critique: P5 H5 E4 S5 R5 V4 · no global styles or page-level macrostructure
 */
import * as React from "react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { C2CAction, C2CCheck, C2CDialog, C2CError, C2C_INPUT } from "@/components/market/c2c-primitives";
import { C2CDescription } from "@/components/market/c2c-description";
import { formatMinorUsd } from "@/lib/market-wallet-money";
import { notifyMarketBalanceChanged } from "@/lib/market-balance-events";
import { C2C_PENDING_PREFIX, c2cDefinitelyRejected, c2cErrorKey, c2cBlockerKey, c2cNow, c2cPayLabel, createC2C, getC2COffers, getC2COrders, getC2CPolicy, matchC2CPendingOrder, parseC2CMinor, quoteC2C, readC2CPending, writeC2CPending, type C2COffer, type C2COrder, type C2CPending, type C2CPolicyView, type C2CQuote } from "@/lib/market-c2c";

export function C2CBuyPanel({ actor, onOrder }: { actor: string; onOrder: (order: C2COrder) => void }) {
  return <C2CBuyWorkspace key={actor} actor={actor} onOrder={onOrder} />;
}
function C2CBuyWorkspace({ actor, onOrder }: { actor: string; onOrder: (order: C2COrder) => void }) {
  const { t, locale } = useLocaleText();
  const [policy, setPolicy] = React.useState<C2CPolicyView | null>(null); const [offers, setOffers] = React.useState<C2COffer[]>([]);
  const [next, setNext] = React.useState<string | null>(null); const [sellerId, setSellerId] = React.useState(""); const [amount, setAmount] = React.useState("");
  const [emptyReason, setEmptyReason] = React.useState<Awaited<ReturnType<typeof getC2COffers>>["emptyReason"]>(null);
  const [quote, setQuote] = React.useState<C2CQuote | null>(null); const [pending, setPending] = React.useState<C2CPending | null>(null);
  const [recoveryBlocked, setRecoveryBlocked] = React.useState(false);
  const [ack, setAck] = React.useState(false); const [busy, setBusy] = React.useState(false); const [error, setError] = React.useState("");
  const [loaded, setLoaded] = React.useState(false); const [now, setNow] = React.useState(c2cNow); const mounted = React.useRef(true); const lock = React.useRef(false);
  const sequence = React.useRef(0); const identity = React.useRef(actor); identity.current = actor;
  const seller = offers.find(s => s.sellerId === sellerId); const minor = parseC2CMinor(amount);
  const valid = !!seller && minor != null && minor >= seller.minMinor && minor <= seller.maxMinor;
  React.useEffect(() => { mounted.current = true; return () => { mounted.current = false; sequence.current++; }; }, [actor]);
  const load = React.useCallback(async (before = "") => {
    const seq = ++sequence.current;
    try {
      const [p, page] = await Promise.all([getC2CPolicy(), getC2COffers(before)]); if (!mounted.current || identity.current !== actor || seq !== sequence.current) return;
      if (page.items.some(s => s.sellerId === actor)) throw new Error("Invalid C2C seller identity");
      setPolicy(p); setOffers(old => before ? [...old, ...page.items.filter(item => !old.some(v => v.sellerId === item.sellerId))] : page.items); setNext(page.next); setLoaded(true); setError("");
      setEmptyReason(page.emptyReason);
    } catch (cause) { if (mounted.current && identity.current === actor && seq === sequence.current) throw cause; }
  }, [actor]);
  React.useEffect(() => {
    setPolicy(null); setOffers([]); setLoaded(false); setSellerId(""); setQuote(null); setAck(false);
    void load().catch(cause => { if (mounted.current) setError(t(c2cErrorKey(cause))); });
    try { const p = readC2CPending(sessionStorage, actor); setPending(p); setRecoveryBlocked(false); if (p) setError(t("c2c.uncertain")); } catch { setRecoveryBlocked(true); setError(t("c2c.storage")); }
  }, [actor, load, t]);
  React.useEffect(() => {
    const refresh = () => { if (document.visibilityState === "visible" && !lock.current && !quote && !pending) void load().catch(cause => { if (mounted.current) setError(t(c2cErrorKey(cause))); }); };
    const timer = setInterval(refresh, 30_000); document.addEventListener("visibilitychange", refresh);
    return () => { clearInterval(timer); document.removeEventListener("visibilitychange", refresh); };
  }, [load, pending, quote, t]);
  React.useEffect(() => { if (!quote) return; const timer = setInterval(() => setNow(c2cNow()), 1000); return () => clearInterval(timer); }, [quote]);
  const run = async (fn: () => Promise<void>) => {
    if (lock.current) return; lock.current = true; setBusy(true); setError("");
    try { await fn(); } catch (cause) { if (mounted.current) setError(t(c2cErrorKey(cause))); }
    finally { lock.current = false; if (mounted.current) setBusy(false); }
  };
  const done = (p: C2CPending, result: C2COrder) => {
    if (!mounted.current || identity.current !== p.actor) return;
    const o = matchC2CPendingOrder(p, result);
    sessionStorage.removeItem(C2C_PENDING_PREFIX + actor); setPending(null); setQuote(null); setAck(false); notifyMarketBalanceChanged(actor); onOrder(o);
  };
  const submit = async (p: C2CPending) => {
    try { done(p, await createC2C(p)); } catch (cause) {
      if (!mounted.current) return;
      if (c2cDefinitelyRejected(cause)) { sessionStorage.removeItem(C2C_PENDING_PREFIX + actor); setPending(null); setQuote(null); throw cause; }
      setError(t("c2c.uncertain"));
    }
  };
  return <div className="grid min-w-0 gap-3" data-c2c-buy>
    <p className="text-sm text-muted-foreground">{t("c2c.intro")}</p>
    <C2CError>{error}</C2CError>
    {recoveryBlocked ? <C2CAction type="button" phase={busy ? "loading" : "default"} onClick={() => void run(async () => {
      const page = await getC2COrders({ role: "buyer", status: "active" });
      if (!mounted.current) return;
      if (page.items.some(o => o.buyerId !== actor) || page.next) throw new Error("C2C recovery needs account review");
      if (page.items[0]) { onOrder(page.items[0]); return; }
      sessionStorage.removeItem(C2C_PENDING_PREFIX + actor); setRecoveryBlocked(false); await load();
    })}>{t("c2c.checkExisting")}</C2CAction> : pending ? <div className="grid justify-items-start gap-2"><p className="text-sm">{t("c2c.uncertain")}</p><C2CAction type="button" phase={busy ? "loading" : "default"} onClick={() => void run(async () => { const page = await getC2COrders({ quoteId: pending.quote.id }); if (page.items[0]) done(pending, page.items[0]); else await submit(pending); })}>{t("c2c.recover")}</C2CAction></div> : <>
      {!loaded ? error ? <C2CAction type="button" secondary disabled={busy} onClick={() => void run(() => load())}>{t("common.reload")}</C2CAction> : <p role="status" className="text-sm text-muted-foreground">{t("common.loading")}</p> : !policy?.available ? <div className="grid justify-items-start gap-2">
        <p role="status" className="text-sm text-muted-foreground">{t(policy?.blockers[0] ? c2cBlockerKey(policy.blockers[0]) : "c2c.error.paused")}</p>
        <C2CAction type="button" secondary disabled={busy} onClick={() => void run(() => load())}>{t("common.reload")}</C2CAction>
      </div> : <>
        <div className="flex flex-wrap items-center justify-between gap-2"><span className="text-sm font-semibold">{t("c2c.buy")}</span><C2CAction type="button" secondary disabled={busy} onClick={() => void run(() => load())}>{t("common.reload")}</C2CAction></div>
        {offers.length ? <div role="group" aria-label={t("c2c.buy")} className="grid max-h-64 min-w-0 gap-2 overflow-y-auto overscroll-contain p-1">
          {offers.map(s => <button key={s.sellerId} type="button" aria-pressed={sellerId === s.sellerId} disabled={busy} onClick={() => { setSellerId(s.sellerId); setAmount(""); setQuote(null); setAck(false); }}
            className={`grid min-h-11 min-w-0 gap-1 rounded-md p-3 text-left focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring active:translate-y-px disabled:opacity-50 motion-reduce:transform-none ${sellerId === s.sellerId ? "bg-primary/10 ring-1 ring-primary" : "bg-muted/50 hover:bg-muted"}`}>
            <span className="flex min-w-0 flex-wrap items-center justify-between gap-2"><span className="min-w-0 break-words text-sm font-medium">{s.displayName}</span><span className="text-xs font-semibold tabular-nums">{t("c2c.rate", { rate: s.priceBps / 100, currency: s.currency })}</span></span>
            <span className="line-clamp-2 break-words text-sm text-muted-foreground [overflow-wrap:anywhere]">{s.description}</span>
            <span className="flex min-w-0 flex-wrap justify-between gap-1 text-xs tabular-nums"><span>{t("c2c.range", { min: formatMinorUsd(s.minMinor), max: formatMinorUsd(s.maxMinor) })}</span><span>{t("c2c.completed", { count: s.completedOrders })}</span></span>
          </button>)}
        </div> : <p role="status" className="text-sm text-muted-foreground">{t(emptyReason ? `c2c.empty.${emptyReason}` : "c2c.empty")}</p>}
        {next ? <C2CAction type="button" secondary disabled={busy} onClick={() => void run(() => load(next))}>{t("common.next")}</C2CAction> : null}
        {seller ? <form className="grid gap-3" onSubmit={e => { e.preventDefault(); if (!valid || !minor) return; void run(async () => { const q = await quoteC2C(seller.sellerId, minor); if (mounted.current) { setQuote(q); setNow(c2cNow()); setAck(false); } }); }}>
          <C2CDescription value={seller.description} />
          <label className="grid min-w-0 gap-1 text-sm">{t("c2c.amount")}<input className={C2C_INPUT} inputMode="decimal" autoComplete="off" value={amount} onChange={e => setAmount(e.target.value)} placeholder={`${seller.minMinor / 100}–${seller.maxMinor / 100}`} disabled={busy} /></label>
          <C2CAction type="submit" disabled={!valid || busy} phase={busy ? "loading" : "default"}>{t("c2c.preview")}</C2CAction>
        </form> : null}
        <p className="text-xs leading-5 text-muted-foreground">{t("c2c.statsHint")} {t("c2c.fee")}</p>
        {!policy.independentDisputeHandlerAvailable ? <details className="text-xs text-muted-foreground"><summary className="min-h-11 cursor-pointer py-3 focus-visible:outline-2 focus-visible:outline-ring">{t("c2c.reviewLimit")}</summary><p>{t("c2c.reviewLimitHelp")}</p></details> : null}
      </>}
    </>}
    <C2CDialog open={!!quote && !pending} title={t("c2c.preview")} busy={busy} onClose={() => setQuote(null)}>
      {quote ? <>
        <p className="break-words text-sm">{quote.sellerName}</p>
        <C2CDescription value={quote.description} links={false} />
        <dl className="grid grid-cols-[minmax(0,1fr)_auto] gap-3 text-sm"><dt>{t("c2c.credit")}</dt><dd className="font-semibold tabular-nums">{formatMinorUsd(quote.creditMinor)}</dd><dt>{t("c2c.pay")}</dt><dd className="font-semibold tabular-nums">{c2cPayLabel(quote.payMinor, quote.currency)}</dd></dl>
        <p className="text-xs text-muted-foreground">{t("c2c.previewExpiry", { time: new Date(quote.expiresAt).toLocaleTimeString(locale) })}</p>
        <p role="status" className="text-sm text-muted-foreground">{t("c2c.applicationHelp", { minutes: quote.paymentMinutes! })}</p>
        <C2CCheck checked={ack} disabled={busy} onChange={e => setAck(e.target.checked)}>{t("c2c.ack")}</C2CCheck><p className="text-xs text-muted-foreground">{t("c2c.fee")}</p>
        <C2CError>{error}</C2CError>
        {now >= Date.parse(quote.expiresAt) ? <>
          <p role="status" className="text-sm text-muted-foreground">{t("c2c.quoteExpired")}</p>
          <C2CAction type="button" secondary phase={busy ? "loading" : "default"} onClick={() => void run(async () => {
            const q = await quoteC2C(quote.sellerId, quote.creditMinor); if (mounted.current) { setQuote(q); setAck(false); setNow(c2cNow()); }
          })}>{t("c2c.requote")}</C2CAction>
        </> : null}
        <C2CAction type="button" phase={busy ? "loading" : "default"} disabled={!ack || busy || recoveryBlocked || now >= Date.parse(quote.expiresAt)} onClick={() => void run(async () => {
          const p = { actor, quote, key: crypto.randomUUID() };
          try { writeC2CPending(sessionStorage, p); } catch { setQuote(null); setRecoveryBlocked(true); setError(t("c2c.storage")); return; }
          setPending(p); await submit(p);
        })}>{t("c2c.create")}</C2CAction>
      </> : null}
    </C2CDialog>
  </div>;
}
