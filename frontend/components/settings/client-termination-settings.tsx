"use client";
import * as React from "react";
import { Button } from "@heroui/react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { ClientTerminationsPanel } from "@/components/market/client-terminations-panel";
import { WALLET_INPUT } from "@/components/market/wallet-primitives";
import { getClientTerminationPolicy, saveClientTerminationPolicy, parseTerminationRate, type ClientTerminationPolicy } from "@/lib/client-termination";

export function ClientTerminationSettings() {
  const { t } = useLocaleText();
  const [policy, setPolicy] = React.useState<ClientTerminationPolicy | null>(null);
  const [rate, setRate] = React.useState("");
  const [error, setError] = React.useState("");
  const [busy, setBusy] = React.useState(false);
  const [saved, setSaved] = React.useState(false);
  const load = React.useCallback(async () => {
    try { const p = await getClientTerminationPolicy(true); setPolicy(p); setRate(String(p.rateBps / 100)); setError(""); }
    catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)); }
  }, []);
  React.useEffect(() => { void load(); }, [load]);
  const save = async (event: React.FormEvent) => {
    event.preventDefault(); if (!policy?.canEdit || busy) return;
    const bps = parseTerminationRate(rate); if (bps === null) { setError(t("clientTermination.invalidRate")); return; }
    setBusy(true); setSaved(false); setError("");
    try { const next = await saveClientTerminationPolicy(policy, bps); setPolicy({ ...next, canEdit: true }); setSaved(true); }
    catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)); }
    finally { setBusy(false); }
  };
  return <div className="grid gap-4"><section className="grid gap-3 rounded-xl border border-border bg-card p-5">
    <h3 className="text-base font-semibold">{t("clientTermination.policyTitle")}</h3><p className="text-sm leading-6 text-muted-foreground">{t("clientTermination.policyHelp")}</p>
    {error ? <p role="alert" className="text-sm text-danger">{error}</p> : null}
    {saved ? <p role="status" className="text-sm text-success">{t("walletPolicy.saved")}</p> : null}
    <form onSubmit={event => void save(event)} className="flex flex-wrap items-end gap-3"><label className="grid gap-1 text-sm">{t("clientTermination.rate")}<input className={WALLET_INPUT} value={rate} inputMode="decimal" onChange={e => {setRate(e.target.value); setSaved(false);}} disabled={!policy?.canEdit || busy} /></label><Button type="submit" variant="primary" isDisabled={!policy?.canEdit || busy}>{t(busy ? "common.loading" : "common.save")}</Button><Button variant="ghost" onPress={() => void load()}>{t("common.reload")}</Button></form>
  </section>{policy?.canEdit ? <ClientTerminationsPanel role="owner" /> : null}</div>;
}
