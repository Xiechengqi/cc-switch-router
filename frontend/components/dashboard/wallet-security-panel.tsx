"use client";
import * as React from "react";
import { Button } from "@heroui/react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { getWalletSecurity, registerWalletPasskey, revokeWalletPasskey, getFinancialRiskPolicy, saveFinancialRiskPolicy, type WalletSecurity, type FinancialRiskPolicy } from "@/lib/wallet-security-api";
import { parseNonNegativeUsdMinor } from "@/lib/market-wallet-money";
const limitFields = ["topupUserDayMinor", "topupChannelDayMinor", "topupGlobalDayMinor", "topupGlobalHourMinor", "incomingUserDayMinor"] as const;
export function WalletSecurityPanel() {
  const { t, locale } = useLocaleText();
  const [view, setView] = React.useState<WalletSecurity | null>(null);
  const [policy, setPolicy] = React.useState<FinancialRiskPolicy | null>(null);
  const [limits, setLimits] = React.useState<Record<string, string>>({});
  const [error, setError] = React.useState(""); const [saved, setSaved] = React.useState(false); const [busy, setBusy] = React.useState(false);
  const alive = React.useRef(true); const lock = React.useRef(false);
  const acceptPolicy = (p: FinancialRiskPolicy) => { setPolicy(p); setLimits(Object.fromEntries(limitFields.map(f => [f, (p[f] / 100).toFixed(2)]))); };
  const load = async () => { const value = await getWalletSecurity(); if (!alive.current) return; setView(value); if (value.canManageRiskPolicy) { const p = await getFinancialRiskPolicy(); if (alive.current) acceptPolicy(p); } };
  const run = async (work: () => Promise<unknown>) => { if (lock.current) return; lock.current = true; setBusy(true); setError(""); setSaved(false); try { await work(); } catch (e) { if (alive.current) setError(e instanceof Error ? e.message : String(e)); } finally { lock.current = false; if (alive.current) setBusy(false); } };
  React.useEffect(() => { alive.current = true; void run(load); return () => { alive.current = false; }; }, []);
  return <details className="rounded-2xl border border-border p-4">
    <summary className="cursor-pointer py-2 font-medium">{t("walletSecurity.title")}</summary>
    <div className="grid min-w-0 gap-3 pt-3">
      <p className="text-sm text-muted-foreground">{t("walletSecurity.help")}</p><p className="text-sm text-muted-foreground">{t("walletSecurity.enrollHelp")}</p>
      {error ? <p role="alert" className="text-sm text-danger">{error}</p> : null}
      <div className="flex flex-wrap gap-2"><Button isDisabled={busy} onPress={() => void run(async () => { await registerWalletPasskey(); await load(); })}>{t("walletSecurity.enroll")}</Button><Button variant="secondary" isDisabled={busy} onPress={() => void run(load)}>{t("common.reload")}</Button></div>
      {view?.passkeys.filter(p => !p.revokedAt).map(p => <div key={p.id} className="flex min-w-0 flex-wrap items-center justify-between gap-2 rounded-xl border border-border p-3"><div className="grid min-w-0 gap-1"><code className="break-all text-xs">{p.id}</code><span className="text-sm">{Date.parse(p.usableAfter) <= Date.now() ? t("walletSecurity.ready") : t("walletSecurity.cooling", { time: new Date(p.usableAfter).toLocaleString(locale) })}</span></div><Button variant="secondary" isDisabled={busy} onPress={() => void run(async () => { await revokeWalletPasskey(p.id); await load(); })}>{t("walletSecurity.revoke")}</Button></div>)}
      <p className="text-xs text-muted-foreground">{t("walletSecurity.revokeHelp")}</p><p className="text-xs text-muted-foreground">{t("walletSecurity.recovery")}</p><code className="select-all break-all text-xs">{view?.sessionReference}</code>
      {policy ? <form className="grid gap-3 border-t border-border pt-4" onSubmit={e => { e.preventDefault(); void run(async () => { const parsed = Object.fromEntries(limitFields.map(f => { const amount = parseNonNegativeUsdMinor(limits[f]); if (amount === null || amount < 100 || amount > 1_000_000_000) throw new Error(t(`walletSecurity.${f}`)); return [f, amount]; })); const p = await saveFinancialRiskPolicy({ ...policy, ...parsed }); if (alive.current) { acceptPolicy(p); setSaved(true); } }); }}>
        <h3 className="font-medium">{t("walletSecurity.riskTitle")}</h3><p className="text-sm text-muted-foreground">{t("walletSecurity.riskHelp")}</p>
        {limitFields.map(f => <label key={f} className="grid gap-1 text-sm">{t(`walletSecurity.${f}`)}<input inputMode="decimal" disabled={busy} className="rounded-lg border border-border bg-background p-2" value={limits[f] || ""} onChange={e => setLimits(v => ({ ...v, [f]: e.target.value }))} required /></label>)}
        <label className="flex items-center gap-2 text-sm"><input type="checkbox" disabled={busy} checked={policy.chainVerificationRequired} onChange={e => setPolicy(p => p && ({ ...p, chainVerificationRequired: e.target.checked }))} />{t("walletSecurity.chainRequired")}</label><p className="text-xs text-muted-foreground">{t("walletSecurity.chainHelp")}</p>
        <label className="flex items-center gap-2 text-sm"><input type="checkbox" disabled={busy} checked={policy.topupCircuitOpen} onChange={e => setPolicy(p => p && ({ ...p, topupCircuitOpen: e.target.checked }))} />{t("walletSecurity.circuitOpen")}</label>
        <Button type="submit" isDisabled={busy} className="justify-self-start">{t("walletSecurity.save")}</Button>{saved ? <p role="status" className="text-sm">{t("walletSecurity.saved")}</p> : null}
      </form> : null}
    </div>
  </details>;
}
