"use client";
/* Hallmark · component: C2C purchase · genre: modern-minimal · theme: existing Router tokens
 * states: default · hover · focus · active · disabled · loading · error · success
 * pre-emit critique: P5 H5 E4 S5 R5 V4 · no global styles or page-level macrostructure
 */
import * as React from "react";
import { ChevronDown, Clock3, RefreshCw } from "lucide-react";
import { InfoHint } from "@/components/common/info-hint";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { C2CAction, C2CCheck, C2CDialog, C2CError, C2C_INPUT } from "@/components/market/c2c-primitives";
import { C2CDescription } from "@/components/market/c2c-description";
import { formatMinorUsd } from "@/lib/market-wallet-money";
import { notifyMarketBalanceChanged } from "@/lib/market-balance-events";
import { C2C_PENDING_PREFIX, c2cDefinitelyRejected, c2cErrorKey, c2cBlockerKey, c2cNow, c2cPayLabel, createC2C, getC2COffers, getC2COrders, getC2CPolicy, matchC2CPendingOrder, parseC2CMinor, quoteC2C, readC2CPending, writeC2CPending, type C2COffer, type C2COrder, type C2CPending, type C2CPolicyView, type C2CQuote } from "@/lib/market-c2c";

// `reloadSignal` hands refreshing to the page; without it the panel shows its own reload.
type C2CBuyProps = { actor: string; onOrder: (order: C2COrder) => void; onBusyChange?: (busy: boolean) => void; inlineConfirmation?: boolean; reloadSignal?: number };
export function C2CBuyPanel(props: C2CBuyProps) {
  return <C2CBuyWorkspace key={props.actor} {...props} />;
}
function C2CBuyWorkspace({ actor, onOrder, onBusyChange, inlineConfirmation = false, reloadSignal }: C2CBuyProps) {
  const { t, locale } = useLocaleText();
  const [policy, setPolicy] = React.useState<C2CPolicyView | null>(null); const [offers, setOffers] = React.useState<C2COffer[]>([]);
  const [next, setNext] = React.useState<string | null>(null); const [sellerId, setSellerId] = React.useState(""); const [amount, setAmount] = React.useState("");
  const [emptyReason, setEmptyReason] = React.useState<Awaited<ReturnType<typeof getC2COffers>>["emptyReason"]>(null);
  const [quote, setQuote] = React.useState<C2CQuote | null>(null); const [pending, setPending] = React.useState<C2CPending | null>(null);
  const [recoveryBlocked, setRecoveryBlocked] = React.useState(false);
  const [ack, setAck] = React.useState(false); const [busy, setBusy] = React.useState(false); const [error, setError] = React.useState("");
  const [loaded, setLoaded] = React.useState(false); const [now, setNow] = React.useState(c2cNow); const mounted = React.useRef(true); const lock = React.useRef(false);
  const sequence = React.useRef(0); const identity = React.useRef(actor); identity.current = actor;
  const previewForm = React.useRef<HTMLFormElement>(null);
  const inlineWasOpen = React.useRef(false);
  const seller = offers.find(s => s.sellerId === sellerId); const minor = parseC2CMinor(amount);
  const valid = !!seller && minor != null && minor >= seller.minMinor && minor <= seller.maxMinor;
  React.useEffect(() => {
    onBusyChange?.(busy || (!!quote && !pending));
    return () => onBusyChange?.(false);
  }, [busy, quote, pending, onBusyChange]);
  React.useEffect(() => {
    if (!inlineConfirmation) return;
    if (quote && !pending) inlineWasOpen.current = true;
    else if (!quote && !pending && !busy && inlineWasOpen.current) {
      inlineWasOpen.current = false;
      previewForm.current?.querySelector<HTMLButtonElement>('button[type="submit"]')?.focus({ preventScroll: true });
    }
  }, [busy, inlineConfirmation, pending, quote]);
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
  const lastReloadSignal = React.useRef(reloadSignal);
  React.useEffect(() => {
    if (reloadSignal === lastReloadSignal.current) return;
    lastReloadSignal.current = reloadSignal;
    if (!lock.current && !quote && !pending) void load().catch(cause => { if (mounted.current) setError(t(c2cErrorKey(cause))); });
  }, [reloadSignal, load, pending, quote, t]);
  const expired = !!quote && now >= Date.parse(quote.expiresAt);
  return <div className="grid min-w-0 gap-3" data-c2c-buy>
    <div className="flex min-w-0 items-start justify-between gap-3">
      <div className="min-w-0">
        <div className="flex items-center gap-1.5"><h3 className="text-sm font-semibold">{t("c2c.sellers")}</h3>
          <InfoHint>
            <span className="block">{t("c2c.intro")}</span>
            <span className="mt-1.5 block">{t("c2c.statsHint")} {t("c2c.fee")}</span>
            {policy && !policy.independentDisputeHandlerAvailable ? <><span className="mt-1.5 block font-medium">{t("c2c.reviewLimit")}</span><span className="block">{t("c2c.reviewLimitHelp")}</span></> : null}
          </InfoHint>
        </div>
        <p className="text-xs text-muted-foreground">{t("c2c.introShort")}</p>
      </div>
      {reloadSignal === undefined && loaded && policy?.available && !quote && !pending && !recoveryBlocked ? <C2CAction type="button" secondary disabled={busy} aria-label={t("common.reload")} onClick={() => void run(() => load())}><RefreshCw className="size-4" aria-hidden="true" /></C2CAction> : null}
    </div>
    <C2CError>{!quote || pending ? error : ""}</C2CError>
    {recoveryBlocked ? <C2CAction type="button" phase={busy ? "loading" : "default"} onClick={() => void run(async () => {
      const page = await getC2COrders({ role: "buyer", status: "active" });
      if (!mounted.current) return;
      if (page.items.some(o => o.buyerId !== actor) || page.next) throw new Error("C2C recovery needs account review");
      if (page.items[0]) { onOrder(page.items[0]); return; }
      sessionStorage.removeItem(C2C_PENDING_PREFIX + actor); setRecoveryBlocked(false); await load();
    })}>{t("c2c.checkExisting")}</C2CAction> : pending ? <div className="grid justify-items-start gap-2"><p className="text-sm">{t("c2c.uncertain")}</p><C2CAction type="button" phase={busy ? "loading" : "default"} onClick={() => void run(async () => { const page = await getC2COrders({ quoteId: pending.quote.id }); if (page.items[0]) done(pending, page.items[0]); else await submit(pending); })}>{t("c2c.recover")}</C2CAction></div> : <>
      <div hidden={inlineConfirmation && !!quote} className={inlineConfirmation && !!quote ? "hidden" : "grid min-w-0 gap-3"}>
      {!loaded ? error ? <C2CAction type="button" secondary className="justify-self-start" disabled={busy} onClick={() => void run(() => load())}>{t("common.reload")}</C2CAction> : <p role="status" className="text-sm text-muted-foreground">{t("common.loading")}</p> : !policy?.available ? <div className="grid justify-items-start gap-2 rounded-xl border border-dashed border-border p-4">
        <p role="status" className="text-sm text-muted-foreground">{t(policy?.blockers[0] ? c2cBlockerKey(policy.blockers[0]) : "c2c.error.paused")}</p>
        <C2CAction type="button" secondary disabled={busy} onClick={() => void run(() => load())}>{t("common.reload")}</C2CAction>
      </div> : <>
        {offers.length ? <div role="group" aria-label={t("c2c.sellers")} className="grid max-h-[32rem] min-w-0 gap-2 overflow-y-auto overscroll-contain p-1">
          {offers.map(s => {
            const selected = sellerId === s.sellerId;
            return <div key={s.sellerId} className={`min-w-0 rounded-xl border transition-all duration-200 ${selected ? "border-accent bg-accent/5 shadow-[0_4px_14px_rgba(0,82,255,0.12)]" : "border-border bg-card hover:border-accent/30 hover:shadow-md"}`}>
              <button type="button" aria-pressed={selected} disabled={busy} onClick={() => { if (selected) return; setSellerId(s.sellerId); setAmount(""); setQuote(null); setAck(false); }}
                className="grid min-h-11 w-full min-w-0 gap-1 rounded-xl p-3 text-left focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring disabled:opacity-50">
                <span className="flex min-w-0 items-center justify-between gap-2">
                  <span className="flex min-w-0 items-center gap-2.5">
                    <span aria-hidden="true" className={`grid size-4 shrink-0 place-items-center rounded-full border-2 ${selected ? "border-accent" : "border-slate-300"}`}>{selected ? <span className="size-1.5 rounded-full bg-accent" /> : null}</span>
                    <span className="min-w-0 truncate text-sm font-semibold">{s.displayName}</span>
                  </span>
                  <span className="shrink-0 rounded-full bg-muted px-2.5 py-0.5 text-xs font-medium tabular-nums text-slate-700">{t("c2c.rate", { rate: s.priceBps / 100, currency: s.currency })}</span>
                </span>
                <span className="pl-[1.625rem] text-xs tabular-nums text-slate-600">{t("c2c.range", { min: formatMinorUsd(s.minMinor), max: formatMinorUsd(s.maxMinor) })} · {t("c2c.completed", { count: s.completedOrders })}</span>
                {!selected && s.description ? <span className="truncate pl-[1.625rem] text-xs text-slate-600">{s.description}</span> : null}
              </button>
              {selected ? <form ref={previewForm} className="grid min-w-0 gap-3 border-t border-accent/15 p-3" onSubmit={e => { e.preventDefault(); if (!valid || !minor) return; void run(async () => { const q = await quoteC2C(s.sellerId, minor); if (mounted.current) { setQuote(q); setNow(c2cNow()); setAck(false); } }); }}>
                <C2CDescription value={s.description} />
                <div className="flex min-w-0 flex-wrap items-end gap-2">
                  <label className="grid min-w-[10rem] flex-1 gap-1 text-sm">{t("c2c.amount")}<input className={C2C_INPUT} inputMode="decimal" autoComplete="off" value={amount} onChange={e => setAmount(e.target.value)} placeholder={`${s.minMinor / 100}–${s.maxMinor / 100}`} disabled={busy} /></label>
                  <C2CAction type="submit" disabled={!valid || busy} phase={busy ? "loading" : "default"}>{t("c2c.preview")}</C2CAction>
                </div>
              </form> : null}
            </div>;
          })}
        </div> : <p role="status" className="rounded-xl border border-dashed border-border p-4 text-sm text-muted-foreground">{t(emptyReason ? `c2c.empty.${emptyReason}` : "c2c.empty")}</p>}
        {next ? <C2CAction type="button" secondary className="justify-self-start" disabled={busy} onClick={() => void run(() => load(next))}>{t("common.next")}</C2CAction> : null}
      </>}
      </div>
    </>}
    <C2CDialog open={!!quote && !pending} inline={inlineConfirmation} title={t("c2c.preview")} busy={busy} onClose={() => setQuote(null)}>
      {quote ? <>
        <p className="-mt-2 break-words text-sm text-slate-600">{quote.sellerName}</p>
        <dl className="grid min-w-0 grid-cols-2 gap-2">
          <div className="min-w-0 rounded-xl bg-muted/60 p-3"><dt className="text-xs text-slate-600">{t("c2c.credit")}</dt><dd className="mt-1 break-all text-xl font-semibold tabular-nums">{formatMinorUsd(quote.creditMinor)}</dd></div>
          <div className="min-w-0 rounded-xl border border-accent/20 bg-accent/5 p-3"><dt className="text-xs text-slate-600">{t("c2c.pay")}</dt><dd className="mt-1 break-all text-xl font-semibold tabular-nums text-accent">{c2cPayLabel(quote.payMinor, quote.currency)}</dd></div>
        </dl>
        <div className="flex min-w-0 flex-wrap items-center gap-2 text-xs">
          <span className="inline-flex items-center gap-1.5 rounded-full bg-muted px-2.5 py-1 tabular-nums text-slate-700"><Clock3 className="size-3.5" aria-hidden="true" />{t("c2c.previewExpiry", { time: new Date(quote.expiresAt).toLocaleTimeString(locale) })}</span>
          <span className="inline-flex items-center rounded-full bg-amber-50 px-2.5 py-1 text-amber-900">{t("c2c.payWithin", { minutes: quote.paymentMinutes! })}</span>
          <InfoHint>{t("c2c.applicationHelp", { minutes: quote.paymentMinutes! })}</InfoHint>
        </div>
        {quote.description ? <details className="group min-w-0 text-sm">
          <summary className="flex min-h-11 cursor-pointer list-none items-center gap-1.5 font-medium focus-visible:outline-2 focus-visible:outline-ring [&::-webkit-details-marker]:hidden"><ChevronDown className="size-4 text-muted-foreground transition-transform group-open:rotate-180 motion-reduce:transition-none" aria-hidden="true" />{t("c2c.paymentNotes")}</summary>
          <C2CDescription value={quote.description} links={false} />
        </details> : null}
        <C2CCheck checked={ack} disabled={busy} onChange={e => setAck(e.target.checked)} hint={t("c2c.ack")}>{t("c2c.ackShort")}</C2CCheck>
        <C2CError>{error}</C2CError>
        {expired ? <div className="grid justify-items-start gap-2">
          <p role="status" className="text-sm text-muted-foreground">{t("c2c.quoteExpired")}</p>
          <C2CAction type="button" secondary phase={busy ? "loading" : "default"} onClick={() => void run(async () => {
            const q = await quoteC2C(quote.sellerId, quote.creditMinor); if (mounted.current) { setQuote(q); setAck(false); setNow(c2cNow()); }
          })}>{t("c2c.requote")}</C2CAction>
        </div> : null}
        <C2CAction type="button" phase={busy ? "loading" : "default"} disabled={!ack || busy || recoveryBlocked || expired} onClick={() => void run(async () => {
          const p = { actor, quote, key: crypto.randomUUID() };
          try { writeC2CPending(sessionStorage, p); } catch { setQuote(null); setRecoveryBlocked(true); setError(t("c2c.storage")); return; }
          setPending(p); await submit(p);
        })}>{t("c2c.create")}</C2CAction>
      </> : null}
    </C2CDialog>
  </div>;
}
