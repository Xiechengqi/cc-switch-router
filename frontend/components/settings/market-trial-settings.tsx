"use client";
import * as React from "react";
import { Button } from "@heroui/react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { WALLET_INPUT } from "@/components/market/wallet-primitives";
import { ApiError } from "@/lib/api";
import { getMarketTrialPolicy, saveMarketTrialPolicy, type MarketTrialPolicy } from "@/lib/market-trial-policy";
import { millionsInputToTokens, tokensToMillionsInput } from "@/lib/token-units";
import { microsToUsdInput, parseUsdLimit } from "@/lib/share-value-quota";

export function MarketTrialSettings() {
  const { t } = useLocaleText();
  const [policy, setPolicy] = React.useState<MarketTrialPolicy | null>(null);
  const [client, setClient] = React.useState("");
  const [share, setShare] = React.useState("");
  const [tokens, setTokens] = React.useState("");
  const [value, setValue] = React.useState("");
  const [error, setError] = React.useState("");
  const [saved, setSaved] = React.useState(false);
  const [busy, setBusy] = React.useState(false);
  const [stale, setStale] = React.useState(false);
  const [attempt, setAttempt] = React.useState(0);
  const load = React.useCallback((p: MarketTrialPolicy) => { setPolicy(p); setClient(String(p.clientHours)); setShare(String(p.shareHours)); setTokens(tokensToMillionsInput(p.shareTokens)); setValue(microsToUsdInput(p.shareValueUsdMicros)); setStale(false); }, []);
  React.useEffect(() => { let active = true; getMarketTrialPolicy(true).then((p) => { if (active) { load(p); setError(""); } }).catch((e) => { if (active) setError(e instanceof Error ? e.message : String(e)); }); return () => { active = false; }; }, [attempt, load]);
  const save = async (event: React.FormEvent) => {
    event.preventDefault(); if (!policy || busy || stale || policy.canEdit === false) return;
    const cap = Number(share) === 0 ? policy.shareTokens : millionsInputToTokens(tokens);
    const parsedValue = parseUsdLimit(value, { allowZero: true });
    if (![client, share].every((n) => /^\d+$/.test(n) && Number(n) <= 720) || cap == null || (Number(share) !== 0 && !parsedValue.ok)) { setError(t("marketTrial.invalid")); return; }
    setBusy(true); setError(""); setSaved(false);
    try { load({ ...await saveMarketTrialPolicy({ ...policy, clientHours: Number(client), shareHours: Number(share), shareTokens: cap, shareValueUsdMicros: Number(share) === 0 ? policy.shareValueUsdMicros : parsedValue.ok ? parsedValue.micros : policy.shareValueUsdMicros }), canEdit: true }); setSaved(true); }
    catch (e) { const changed = e instanceof ApiError && e.status === 409; setStale(changed); setError(changed ? t("walletPolicy.changed") : e instanceof Error ? e.message : String(e)); }
    finally { setBusy(false); }
  };
  const disabled = busy || !policy || policy.canEdit === false;
  return <section className="grid min-w-0 gap-4 rounded-xl border border-border bg-card p-5" aria-labelledby="market-trial-title">
    <div><h3 id="market-trial-title" className="text-base font-semibold">{t("marketTrial.title")}</h3><p className="mt-1 text-sm leading-6 text-muted-foreground">{t("marketTrial.help")}</p></div>
    {error ? <p role="alert" className="text-sm text-danger">{error}</p> : null}
    {saved ? <p role="status" className="text-sm text-success">{t("walletPolicy.saved")}</p> : null}
    {!policy || stale ? <Button variant="secondary" onPress={() => setAttempt((n) => n + 1)}>{t("settings.marketFinance.refresh")}</Button> : null}
    <form onSubmit={(e) => void save(e)} className="grid gap-4 sm:grid-cols-2">
      <label className="grid gap-1 text-sm">{t("walletPolicy.clientTrialHours")}<input aria-label={t("walletPolicy.clientTrialHours")} className={WALLET_INPUT} type="number" min={0} max={720} step={1} required value={client} onChange={(e) => { setClient(e.target.value); setSaved(false); }} disabled={disabled} /></label>
      <label className="grid gap-1 text-sm">{t("marketTrial.shareHours")}<input aria-label={t("marketTrial.shareHours")} className={WALLET_INPUT} type="number" min={0} max={720} step={1} required value={share} onChange={(e) => { setShare(e.target.value); setSaved(false); }} disabled={disabled} /></label>
      <label className="grid gap-1 text-sm">{t("marketTrial.shareTokens")}<input aria-label={t("marketTrial.shareTokens")} className={WALLET_INPUT} inputMode="decimal" required value={tokens} onChange={(e) => { setTokens(e.target.value); setSaved(false); }} disabled={disabled || share === "0"} /></label>
      <label className="grid gap-1 text-sm">{t("marketTrial.shareValue")}<span className="relative block"><span aria-hidden="true" className="pointer-events-none absolute left-3 top-1/2 -translate-y-1/2 text-muted-foreground">$</span><input aria-label={t("marketTrial.shareValue")} className={`${WALLET_INPUT} w-full pl-6`} inputMode="decimal" required value={value} onChange={(e) => { setValue(e.target.value); setSaved(false); }} disabled={disabled || share === "0"} /></span></label>
      <p className="text-xs leading-6 text-muted-foreground sm:col-span-2">{t("marketTrial.rules")}</p>
      <p className="text-xs leading-6 text-muted-foreground sm:col-span-2">{t("clientTrial.settings")}</p>
      <Button type="submit" variant="primary" isDisabled={disabled || stale}>{busy ? t("common.loading") : t("common.save")}</Button>
    </form>
  </section>;
}
