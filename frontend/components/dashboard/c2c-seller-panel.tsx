"use client";
/* Hallmark · component: C2C seller editor · genre: modern-minimal · theme: existing Router tokens
 * audience: wallet holders · use: publish a concise offer · tone: factual
 * states: default · hover · focus · active · disabled · loading · error · success
 * pre-emit critique: P5 H5 E4 S5 R5 V4 · component-scope: no macrostructure
 */
import * as React from "react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { C2CAction, C2CCheck, C2CError, C2C_INPUT } from "@/components/market/c2c-primitives";
import { C2C_DESCRIPTION_MAX, c2cErrorKey, c2cBlockerKey, parseC2CMinor, saveC2CSeller, validC2CDescription, type C2CPolicyView, type C2CSeller, type C2CSellerSnapshot } from "@/lib/market-c2c";
import { formatMinorUsd, formatWalletUnits, parseNonNegativeUsdMinor } from "@/lib/market-wallet-money";

// A mounted editor owns its draft. Workspace polling never replaces these
// values or advances the version the user reviewed.
export function C2CSellerPanel({ policy, snapshot, availabilityError, onRefresh, onSaved, onBusyChange, onReload }: {
  policy: C2CPolicyView; snapshot: C2CSellerSnapshot;
  availabilityError: boolean; onRefresh: () => Promise<void>;
  onSaved: (seller: C2CSeller) => void; onBusyChange: (busy: boolean) => void; onReload: () => Promise<void>;
}) {
  const { t } = useLocaleText();
  const [original] = React.useState(snapshot.seller);
  const defaultMin = Math.max(policy.policy.minMinor, Math.min(100, policy.policy.maxMinor));
  const defaultMax = Math.max(defaultMin, Math.min(10000, policy.policy.maxMinor));
  const [description, setDescription] = React.useState(original?.description ?? "");
  const [currency, setCurrency] = React.useState<"USD" | "CNY">(original?.currency ?? "USD");
  const [price, setPrice] = React.useState(String((original?.priceBps ?? 10000) / 100));
  const [min, setMin] = React.useState(String((original?.minMinor ?? defaultMin) / 100));
  const [max, setMax] = React.useState(String((original?.maxMinor ?? defaultMax) / 100));
  const [ack, setAck] = React.useState(false);
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState("");
  const [stale, setStale] = React.useState(false);
  const [touched, setTouched] = React.useState(false);
  const alive = React.useRef(true); const lock = React.useRef(false);
  const fieldId = React.useId();
  React.useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);
  const low = parseC2CMinor(min); const high = parseC2CMinor(max); const bps = parseNonNegativeUsdMinor(price);
  const descriptionValid = validC2CDescription(description.trim());
  const termsValid = low != null && high != null && high >= low && bps != null && bps >= 5000 && bps <= 20000;
  const versionChanged = (snapshot.seller?.version ?? 0) !== (original?.version ?? 0);
  const blocked = availabilityError ? "c2c.eligibilityFailed" : snapshot.blockers[0] ? c2cBlockerKey(snapshot.blockers[0]) : null;
  const save = async () => {
    if (lock.current || versionChanged || !descriptionValid || !termsValid || !ack || blocked || low == null || high == null || bps == null) return;
    lock.current = true; setBusy(true); onBusyChange(true); setError(""); setStale(false);
    try {
      const seller = await saveC2CSeller({
        expectedVersion: original?.version ?? 0, enabled: true, description: description.trim(),
        currency, priceBps: bps, minMinor: low, maxMinor: high, acknowledgeExternalPayment: ack,
      });
      if (alive.current) onSaved(seller);
    } catch (cause) {
      if (alive.current) { setError(t(c2cErrorKey(cause))); setStale(c2cErrorKey(cause) === "c2c.error.stale"); }
    } finally {
      lock.current = false;
      // Parent clears busy before unmounting after a successful save.
      if (alive.current) { setBusy(false); onBusyChange(false); }
    }
  };
  const reload = async () => {
    if (lock.current) return;
    lock.current = true; setBusy(true); onBusyChange(true); setError("");
    try { await onReload(); }
    catch (cause) { if (alive.current) setError(t(c2cErrorKey(cause))); }
    finally { lock.current = false; if (alive.current) { setBusy(false); onBusyChange(false); } }
  };
  const refreshEligibility = async () => {
    if (lock.current) return;
    lock.current = true; setBusy(true); onBusyChange(true); setError("");
    try { await onRefresh(); }
    catch { if (alive.current) setError(t("c2c.eligibilityFailed")); }
    finally { lock.current = false; if (alive.current) { setBusy(false); onBusyChange(false); } }
  };
  return <form className="grid min-w-0 gap-4" onSubmit={event => { event.preventDefault(); setTouched(true); void save(); }} data-c2c-seller-editor>
    <p className="text-sm tabular-nums text-muted-foreground">{t("c2c.available", { amount: formatWalletUnits(snapshot.availableUnits) })}</p>
    <p className="text-sm tabular-nums text-muted-foreground">{t("c2c.qualification", { balance: formatWalletUnits(snapshot.admissionBalanceUnits), threshold: formatMinorUsd(policy.policy.admissionMinor) })}</p>
    <div className="grid min-w-0 gap-1">
      <label htmlFor={fieldId} className="text-sm font-medium">{t("c2c.description")}</label>
      <input id={fieldId} autoFocus className={C2C_INPUT} value={description} maxLength={C2C_DESCRIPTION_MAX}
        placeholder={t("c2c.descriptionExample")} required disabled={busy} aria-invalid={touched && !descriptionValid}
        aria-describedby={fieldId + "-help"} onBlur={() => setTouched(true)} onChange={event => setDescription(event.target.value)} />
      <p id={fieldId + "-help"} className={`min-h-[1lh] text-xs ${touched && !descriptionValid ? "text-destructive" : "text-muted-foreground"}`}>
        {t(touched && !descriptionValid ? "c2c.descriptionError" : "c2c.descriptionHelp")}
      </p>
    </div>
    <details className="min-w-0">
      <summary className="min-h-11 cursor-pointer rounded-md py-2 text-sm focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring">
        <span className="font-medium">{t("c2c.tradeSettings")}</span>
        <span className="mt-1 block text-xs text-muted-foreground">{t("c2c.rate", { rate: bps != null ? bps / 100 : "—", currency })} · {t("c2c.range", { min: low != null ? formatMinorUsd(low) : "—", max: high != null ? formatMinorUsd(high) : "—" })}</span>
      </summary>
      <div className="grid min-w-0 gap-3 pt-3 sm:grid-cols-2">
        <label className="grid min-w-0 gap-1 text-sm">{t("c2c.currency")}<select className={C2C_INPUT} value={currency} disabled={busy} onChange={event => setCurrency(event.target.value as "USD" | "CNY")}><option value="USD">USD</option><option value="CNY">CNY</option></select></label>
        <label className="grid min-w-0 gap-1 text-sm">{t("c2c.price")}<input className={C2C_INPUT} inputMode="decimal" value={price} disabled={busy} onChange={event => setPrice(event.target.value)} required /></label>
        <label className="grid min-w-0 gap-1 text-sm">{t("c2c.min")}<input className={C2C_INPUT} inputMode="decimal" value={min} disabled={busy} onChange={event => setMin(event.target.value)} required /></label>
        <label className="grid min-w-0 gap-1 text-sm">{t("c2c.max")}<input className={C2C_INPUT} inputMode="decimal" value={max} disabled={busy} onChange={event => setMax(event.target.value)} required /></label>
      </div>
      <p className="mt-2 text-xs text-muted-foreground">{t("c2c.currencyHelp")}</p>
    </details>
    {!termsValid ? <p role="alert" className="text-sm text-destructive">{t("c2c.termsError")}</p> : null}
    <C2CCheck checked={ack} disabled={busy} onChange={event => setAck(event.target.checked)}>{t("c2c.sellerAck")}</C2CCheck>
    <div id={fieldId + "-eligibility"} aria-live="polite" className="grid min-w-0 gap-2 text-sm text-muted-foreground">
      {blocked ? <p>{t(blocked)}</p> : null}
      {!availabilityError && snapshot.blockers.length > 1 ? <details><summary className="min-h-11 cursor-pointer rounded-md py-2 focus-visible:outline-2 focus-visible:outline-ring">{t("c2c.moreBlockers")}</summary><ul className="grid gap-2">{snapshot.blockers.slice(1).map(code => <li key={code}>{t(c2cBlockerKey(code))}</li>)}</ul></details> : null}
      {policy.canManagePolicy && snapshot.blockers.some(code => ["C2C_DISABLED", "C2C_TRANSFERS_DISABLED", "C2C_TRANSFERS_PAUSED", "C2C_EMAIL_UNAVAILABLE"].includes(code)) ? <a href="/settings/?panel=market_finance" className="inline-flex min-h-11 items-center justify-self-start whitespace-nowrap font-medium text-primary underline underline-offset-2 focus-visible:outline-2 focus-visible:outline-ring">{t("c2c.managePolicy")}</a> : null}
    </div>
    {versionChanged ? <p role="status" className="text-sm text-muted-foreground">{t("c2c.error.stale")}</p> : null}
    <C2CError>{error}</C2CError>
    <div className="flex flex-wrap items-center justify-end gap-2">
      {stale || versionChanged ? <C2CAction type="button" secondary disabled={busy} onClick={() => void reload()}>{t("c2c.reloadSettings")}</C2CAction>
        : blocked ? <C2CAction type="button" secondary phase={busy ? "loading" : "default"} disabled={busy} onClick={() => void refreshEligibility()}>{t("c2c.refreshEligibility")}</C2CAction> : null}
      <C2CAction type="submit" aria-describedby={blocked ? fieldId + "-eligibility" : undefined} phase={busy ? "loading" : "default"} disabled={!descriptionValid || !termsValid || !ack || !!blocked || busy || stale || versionChanged}>
        {t(original?.enabled ? "common.save" : "c2c.startSelling")}
      </C2CAction>
    </div>
  </form>;
}
