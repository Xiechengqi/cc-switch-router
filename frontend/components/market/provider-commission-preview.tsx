"use client";
import * as React from "react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { getCommissionPolicy, type CommissionPolicy } from "@/lib/wallet-policy-api";
import { commissionPreview } from "@/lib/market-commission";
import { formatWalletUnits } from "@/lib/market-wallet-money";

export function ProviderCommissionPreview({ price }: { price: string }) {
  const { locale, t } = useLocaleText();
  const [policy, setPolicy] = React.useState<CommissionPolicy | null>(null);
  const [failed, setFailed] = React.useState(false);
  React.useEffect(() => {
    let active = true;
    const reload = () => getCommissionPolicy().then((next) => { if (active) { setPolicy(next); setFailed(false); } }).catch(() => { if (active) setFailed(true); });
    void reload();
    const timer = window.setInterval(() => void reload(), 60000);
    return () => { active = false; window.clearInterval(timer); };
  }, []);
  const preview = policy && commissionPreview(price, policy.current.rateBps);
  if (failed) return <p role="status" className="text-sm text-muted-foreground">{t("commission.unavailable")}</p>;
  if (!preview || !policy) return null;
  return <div className="grid min-w-0 gap-1 text-sm" aria-live="polite">
    <p className="flex flex-wrap gap-x-3 gap-y-1 tabular-nums">
      <span className="text-muted-foreground">{t("commission.previewFee", { rate: (policy.current.rateBps / 100).toString(), fee: formatWalletUnits(preview.feeUnits, "$", locale) })}</span>
      <span className="font-medium">{t("commission.previewNet", { amount: formatWalletUnits(preview.netUnits, "$", locale) })}</span>
    </p>
    {policy.scheduled ? <p className="text-muted-foreground">{t("commission.scheduled", { rate: (policy.scheduled.rateBps / 100).toString(), time: new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short" }).format(new Date(policy.scheduled.effectiveAt)) })}</p> : null}
  </div>;
}
