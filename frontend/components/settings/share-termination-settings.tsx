"use client";
import * as React from "react";
import { Button } from "@heroui/react";
import { useAuth } from "@/components/auth/auth-provider";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { WALLET_INPUT } from "@/components/market/wallet-primitives";
import { parseTerminationRate } from "@/lib/client-termination";
import { getShareTerminationPolicy, saveShareTerminationPolicy, type ShareTerminationPolicy } from "@/lib/share-termination";

export function ShareTerminationSettings() {
  const { session } = useAuth(); const actor = session?.user?.id;
  return actor ? <SettingsContent key={actor} /> : null;
}
function SettingsContent() {
  const { t } = useLocaleText();
  const [policy, setPolicy] = React.useState<ShareTerminationPolicy | null>(null);
  const [rate, setRate] = React.useState(""); const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState(""); const [saved, setSaved] = React.useState(false);
  const c = React.useRef<AbortController | null>(null);
  const working = React.useRef(false);
  const load = React.useCallback(async () => {
    if (working.current) return; c.current?.abort(); const abort = new AbortController(); c.current = abort;
    try { const p = await getShareTerminationPolicy(abort.signal); if (!abort.signal.aborted) { setPolicy(p); setRate(String(p.rateBps / 100)); setError(""); } }
    catch (e) { if (!abort.signal.aborted) setError(e instanceof Error ? e.message : String(e)); }
  }, []);
  React.useEffect(() => { void load(); return () => c.current?.abort(); }, [load]);
  const save = async (e: React.FormEvent) => {
    e.preventDefault(); if (!policy?.canEdit || working.current) return;
    const bps = parseTerminationRate(rate); if (bps === null) { setError(t("clientTermination.invalidRate")); return; }
    c.current?.abort(); const abort = new AbortController(); c.current = abort;
    working.current = true; setBusy(true); setSaved(false); setError("");
    try { const next = await saveShareTerminationPolicy(policy, bps, abort.signal); if (!abort.signal.aborted) { setPolicy({ ...next, canEdit: true }); setSaved(true); } }
    catch (e) { if (!abort.signal.aborted) { setError(e instanceof Error ? e.message : String(e)); setPolicy(p => p ? { ...p, canEdit: false } : null); } }
    finally { working.current = false; if (!abort.signal.aborted) setBusy(false); }
  };
  return <section className="grid gap-3 rounded-xl border border-border bg-card p-5">
    <h3 className="text-base font-semibold">{t("shareTermination.policyTitle")}</h3><p className="text-sm leading-6 text-muted-foreground">{t("shareTermination.policyHelp")}</p>
    {error ? <p role="alert" className="text-sm text-danger">{error}</p> : null}{saved ? <p role="status" className="text-sm text-success">{t("walletPolicy.saved")}</p> : null}
    <form className="flex flex-wrap items-end gap-3" onSubmit={e => void save(e)}><label className="grid gap-1 text-sm">{t("clientTermination.rate")}<input className={WALLET_INPUT} inputMode="decimal" value={rate} disabled={!policy?.canEdit || busy} onChange={e => { setRate(e.target.value); setSaved(false); }} /></label><Button type="submit" variant="primary" isDisabled={!policy?.canEdit || busy}>{t(busy ? "common.loading" : "common.save")}</Button><Button variant="ghost" isDisabled={busy} onPress={() => void load()}>{t("common.reload")}</Button></form>
  </section>;
}
