"use client";
/* Hallmark · component: C2C seller settings · genre: modern-minimal · theme: existing Router tokens
 * states: default · hover · focus · active · disabled · loading · error · success
 * pre-emit critique: P5 H4 E4 S5 R5 V4
 */
import * as React from "react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { C2CAction, C2CCheck, C2CError, C2C_INPUT } from "@/components/market/c2c-primitives";
import { c2cErrorKey, getC2CSeller, heartbeatC2C, parseC2CMinor, saveC2CSeller, type C2CPolicyView, type C2CSeller } from "@/lib/market-c2c";
import { formatMinorUsd, formatWalletUnits, parseNonNegativeUsdMinor } from "@/lib/market-wallet-money";

export function C2CSellerPanel({ policy, onSaved }: { policy: C2CPolicyView; onSaved: () => void }) {
  const { t } = useLocaleText(); const [seller, setSeller] = React.useState<C2CSeller | null>(null);
  const [loaded, setLoaded] = React.useState(false); const [enabled, setEnabled] = React.useState(false); const [name, setName] = React.useState("");
  const [contact, setContact] = React.useState(""); const [method, setMethod] = React.useState(""); const [details, setDetails] = React.useState("");
  const [currency, setCurrency] = React.useState<"USD" | "CNY">("USD"); const [price, setPrice] = React.useState("100"); const [min, setMin] = React.useState("1"); const [max, setMax] = React.useState("100");
  const [stock, setStock] = React.useState("0"); const [ack, setAck] = React.useState(false); const [busy, setBusy] = React.useState(false); const [error, setError] = React.useState(""); const [saved, setSaved] = React.useState(false);
  const alive = React.useRef(true); const lock = React.useRef(false);
  React.useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);
  const load = React.useCallback(async () => {
    const result = await getC2CSeller(); if (!alive.current) return;
    const s = result.seller; setSeller(s); setStock(result.availableUnits); setLoaded(true);
    if (s) { setEnabled(s.enabled); setName(s.displayName); setContact(s.contact); setMethod(s.paymentMethod); setDetails(s.paymentDetails); setCurrency(s.currency); setPrice(String(s.priceBps / 100)); setMin(String(s.minMinor / 100)); setMax(String(s.maxMinor / 100)); }
  }, []);
  React.useEffect(() => { void load().catch(e => { if (alive.current) setError(t(c2cErrorKey(e))); }); }, [load, t]);
  React.useEffect(() => {
    if (!seller?.enabled || seller.suspended) return;
    const beat = () => { if (document.visibilityState === "visible") void heartbeatC2C().catch(e => { if (alive.current) setError(t(c2cErrorKey(e))); }); };
    beat(); const timer = setInterval(beat, 60_000); document.addEventListener("visibilitychange", beat);
    return () => { clearInterval(timer); document.removeEventListener("visibilitychange", beat); };
  }, [seller?.enabled, seller?.suspended, t]);
  const low = parseC2CMinor(min); const high = parseC2CMinor(max); const bps = parseNonNegativeUsdMinor(price);
  const valid = !!name.trim() && !!contact.trim() && !!method.trim() && !!details.trim() && low != null && high != null && high >= low && bps != null && bps >= 5000 && bps <= 20000;
  return <form className="grid min-w-0 gap-4" onSubmit={e => {
    e.preventDefault(); if (lock.current || !valid || !ack || low == null || high == null || bps == null) return;
    lock.current = true; setBusy(true); setError(""); setSaved(false);
    void saveC2CSeller({ expectedVersion: seller?.version ?? 0, enabled, displayName: name.trim(), contact: contact.trim(), paymentMethod: method.trim(), paymentDetails: details.trim(), currency, priceBps: bps, minMinor: low, maxMinor: high, acknowledgeExternalPayment: true })
      .then(s => { if (alive.current) { setSeller(s); setSaved(true); setAck(false); onSaved(); } })
      .catch(cause => { if (alive.current) setError(t(c2cErrorKey(cause))); })
      .finally(() => { lock.current = false; if (alive.current) setBusy(false); });
  }}>
    <p className="text-sm text-muted-foreground">{t("c2c.sellerHelp", { amount: formatMinorUsd(policy.policy.admissionMinor) })}</p>
    <p className="text-sm tabular-nums">{t("c2c.available", { amount: formatWalletUnits(stock) })}{seller?.suspended ? ` · ${t("c2c.suspended")}` : ""}</p>
    <C2CError>{error}</C2CError>
    {!loaded ? <C2CAction type="button" secondary onClick={() => void load().catch(cause => setError(t(c2cErrorKey(cause))))}>{t("common.reload")}</C2CAction> : <>
      <div className="grid min-w-0 gap-3 sm:grid-cols-2">
        <label className="grid min-w-0 gap-1 text-sm">{t("c2c.name")}<input className={C2C_INPUT} value={name} maxLength={60} disabled={busy} onChange={e => { setName(e.target.value); setSaved(false); }} required /></label>
        <label className="grid min-w-0 gap-1 text-sm">{t("c2c.method")}<input className={C2C_INPUT} value={method} maxLength={60} disabled={busy} onChange={e => { setMethod(e.target.value); setSaved(false); }} required /></label>
        <label className="grid min-w-0 gap-1 text-sm">{t("c2c.contact")}<textarea className={C2C_INPUT} value={contact} rows={2} maxLength={300} disabled={busy} onChange={e => { setContact(e.target.value); setSaved(false); }} required /></label>
        <label className="grid min-w-0 gap-1 text-sm">{t("c2c.instructions")}<textarea className={C2C_INPUT} value={details} rows={2} maxLength={1500} disabled={busy} onChange={e => { setDetails(e.target.value); setSaved(false); }} required /></label>
        <label className="grid min-w-0 gap-1 text-sm">{t("c2c.currency")}<select className={C2C_INPUT} value={currency} disabled={busy} onChange={e => { setCurrency(e.target.value as "USD" | "CNY"); setSaved(false); }}><option value="USD">USD</option><option value="CNY">CNY</option></select></label>
        <label className="grid min-w-0 gap-1 text-sm">{t("c2c.price")}<input className={C2C_INPUT} inputMode="decimal" value={price} disabled={busy} onChange={e => { setPrice(e.target.value); setSaved(false); }} required /></label>
        <label className="grid min-w-0 gap-1 text-sm">{t("c2c.min")}<input className={C2C_INPUT} inputMode="decimal" value={min} disabled={busy} onChange={e => { setMin(e.target.value); setSaved(false); }} required /></label>
        <label className="grid min-w-0 gap-1 text-sm">{t("c2c.max")}<input className={C2C_INPUT} inputMode="decimal" value={max} disabled={busy} onChange={e => { setMax(e.target.value); setSaved(false); }} required /></label>
      </div>
      <p className="text-xs text-muted-foreground">{t("c2c.private")}</p>
      <C2CCheck checked={enabled} disabled={busy || !!seller?.suspended || (!policy.available && !seller?.enabled)} onChange={e => { setEnabled(e.target.checked); setSaved(false); }}>{t("c2c.enabled")}</C2CCheck>
      {seller?.enabled ? <p className="text-xs text-muted-foreground">{t("c2c.sellerOnline")}</p> : null}
      <C2CCheck checked={ack} disabled={busy} onChange={e => setAck(e.target.checked)}>{t("c2c.sellerAck")}</C2CCheck>
      <div className="flex flex-wrap items-center gap-3"><C2CAction type="submit" phase={busy ? "loading" : saved ? "success" : "default"} disabled={!valid || !ack || busy}>{t("common.save")}</C2CAction><C2CAction type="button" secondary disabled={busy} onClick={() => void load().catch(cause => setError(t(c2cErrorKey(cause))))}>{t("common.reload")}</C2CAction>{saved ? <span role="status" className="text-sm text-muted-foreground">{t("c2c.saved")}</span> : null}</div>
    </>}
  </form>;
}
