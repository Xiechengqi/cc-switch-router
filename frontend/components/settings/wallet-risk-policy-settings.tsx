"use client";

/* Owner-only top-up limits and the top-up circuit breaker. Moved out of the
 * buyer's balance page: only the Router Owner can change them, and saving is
 * confirmed with the Owner's login password like every finance write. */

import * as React from "react";
import { Button } from "@heroui/react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { ApiError } from "@/lib/api";
import { WALLET_INPUT } from "@/components/market/wallet-primitives";
import { parseNonNegativeUsdMinor } from "@/lib/market-wallet-money";
import { walletErrorText } from "@/lib/wallet-errors";
import { getFinancialRiskPolicy, saveFinancialRiskPolicy, type FinancialRiskPolicy } from "@/lib/wallet-security-api";

const LIMIT_FIELDS = ["topupUserDayMinor", "topupChannelDayMinor", "topupGlobalDayMinor", "topupGlobalHourMinor", "incomingUserDayMinor"] as const;

export function WalletRiskPolicySettings() {
  const { t, locale } = useLocaleText();
  const [policy, setPolicy] = React.useState<FinancialRiskPolicy | null>(null);
  const [limits, setLimits] = React.useState<Record<string, string>>({});
  const [error, setError] = React.useState("");
  const [saved, setSaved] = React.useState(false);
  const [busy, setBusy] = React.useState(false);
  const [hidden, setHidden] = React.useState(false);
  const accept = (next: FinancialRiskPolicy) => {
    setPolicy(next);
    setLimits(Object.fromEntries(LIMIT_FIELDS.map((field) => [field, (next[field] / 100).toFixed(2)])));
  };
  React.useEffect(() => {
    let active = true;
    getFinancialRiskPolicy().then((value) => { if (active) accept(value); }).catch((cause) => {
      if (!active) return;
      // Only the Router Owner manages these limits; other admins see nothing.
      if (cause instanceof ApiError && (cause.status === 401 || cause.status === 403)) setHidden(true);
      else setError(walletErrorText(cause, t, locale));
    });
    return () => { active = false; };
  }, [t, locale]);

  const save = async (event: React.FormEvent) => {
    event.preventDefault();
    if (!policy || busy) return;
    setBusy(true);
    setError("");
    setSaved(false);
    try {
      const parsed: Partial<FinancialRiskPolicy> = {};
      for (const field of LIMIT_FIELDS) {
        const amount = parseNonNegativeUsdMinor(limits[field] ?? "");
        if (amount === null || amount < 100 || amount > 1_000_000_000) throw new Error(t(`walletSecurity.${field}`));
        parsed[field] = amount;
      }
      accept(await saveFinancialRiskPolicy({ ...policy, ...parsed }));
      setSaved(true);
    } catch (cause) {
      setError(walletErrorText(cause, t, locale));
      // Someone else saved first. Keep the Owner's draft and adopt the
      // current version, so saving again is a deliberate overwrite.
      if (cause instanceof ApiError && cause.status === 409) {
        await getFinancialRiskPolicy()
          .then((latest) => setPolicy((current) => current && { ...current, version: latest.version }))
          .catch(() => {});
      }
    } finally {
      setBusy(false);
    }
  };

  if (hidden || (!policy && !error)) return null;
  return (
    <section className="grid min-w-0 gap-4 rounded-xl border border-border bg-card p-5">
      <div>
        <h3 className="text-base font-semibold">{t("walletSecurity.riskTitle")}</h3>
        <p className="mt-1 text-sm leading-6 text-muted-foreground">{t("walletSecurity.riskHelp")}</p>
      </div>
      {error ? <p role="alert" className="text-sm text-destructive">{error}</p> : null}
      {policy ? (
        <form onSubmit={(event) => void save(event)} className="grid min-w-0 gap-4">
          <div className="grid min-w-0 gap-4 sm:grid-cols-2">
            {LIMIT_FIELDS.map((field) => (
              <label key={field} className="grid min-w-0 gap-1 text-sm">
                {t(`walletSecurity.${field}`)}
                <input inputMode="decimal" disabled={busy} className={WALLET_INPUT} value={limits[field] || ""}
                  onChange={(event) => setLimits((value) => ({ ...value, [field]: event.target.value }))} required />
              </label>
            ))}
          </div>
          <label className="flex min-h-11 items-start gap-3 text-sm">
            <input type="checkbox" className="mt-1 size-4 accent-primary" disabled={busy} checked={policy.chainVerificationRequired}
              onChange={(event) => setPolicy((value) => value && ({ ...value, chainVerificationRequired: event.target.checked }))} />
            <span className="grid gap-0.5">{t("walletSecurity.chainRequired")}<span className="text-xs text-muted-foreground">{t("walletSecurity.chainHelp")}</span></span>
          </label>
          <label className="flex min-h-11 items-center gap-3 text-sm">
            <input type="checkbox" className="size-4 accent-primary" disabled={busy} checked={policy.topupCircuitOpen}
              onChange={(event) => setPolicy((value) => value && ({ ...value, topupCircuitOpen: event.target.checked }))} />
            {t("walletSecurity.circuitOpen")}
          </label>
          <div className="flex flex-wrap items-center gap-3">
            <Button type="submit" variant="primary" isDisabled={busy}>{t("walletSecurity.save")}</Button>
            {saved ? <p role="status" className="text-sm text-emerald-700">{t("walletSecurity.saved")}</p> : null}
          </div>
        </form>
      ) : null}
    </section>
  );
}
