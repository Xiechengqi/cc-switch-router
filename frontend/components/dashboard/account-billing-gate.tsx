"use client";

import * as React from "react";
import { useRouter } from "next/navigation";
import { Button } from "@heroui/react";
import { useAuth } from "@/components/auth/auth-provider";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { AccountBillingPage } from "@/components/dashboard/account-billing-page";
import { DASHBOARD_ACCOUNT_WALLET_PATH } from "@/lib/dashboard-nav";
import { getMarketWallet } from "@/lib/market-wallet-api";

export function AccountBillingGate() {
  const { t } = useLocaleText();
  const router = useRouter();
  const { session, loading } = useAuth();
  const actor = session?.user?.id || session?.user?.email || "authenticated";
  const [result, setResult] = React.useState<{ actor: string; mode: string } | null>(null);
  const [error, setError] = React.useState("");
  const [attempt, setAttempt] = React.useState(0);

  React.useEffect(() => {
    if (!session?.authenticated) return;
    let active = true;
    getMarketWallet().then((wallet) => {
      if (!active) return;
      setResult({ actor, mode: wallet.mode });
      setError("");
      if (wallet.mode === "enabled") router.replace(DASHBOARD_ACCOUNT_WALLET_PATH);
    }).catch((cause) => {
      if (active) setError(cause instanceof Error ? cause.message : String(cause));
    });
    return () => { active = false; };
  }, [actor, attempt, router, session?.authenticated]);

  if (loading) return <p className="p-6 text-sm text-muted-foreground">{t("common.loading")}</p>;
  if (!session?.authenticated) return <AccountBillingPage />;
  if (error) return <div className="grid justify-items-start gap-3 p-6"><p role="alert" className="text-sm text-destructive">{error}</p><Button variant="outline" onPress={() => { setError(""); setAttempt((value) => value + 1); }}>{t("common.retry")}</Button></div>;
  if (!result || result.actor !== actor || result.mode === "enabled") return <p className="p-6 text-sm text-muted-foreground">{t("common.loading")}</p>;
  return <AccountBillingPage />;
}
