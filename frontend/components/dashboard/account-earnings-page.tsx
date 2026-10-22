"use client";
import * as React from "react";
import { useAuth } from "@/components/auth/auth-provider";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { getMarketWallet } from "@/lib/market-wallet-api";
import { ProviderFinancePanel } from "@/components/dashboard/provider-finance-panel";

export function AccountEarningsPage() {
  const { session } = useAuth();
  const { t } = useLocaleText();
  const [mode, setMode] = React.useState<
    "disabled" | "shadow" | "enabled" | null
  >(null);
  const [error, setError] = React.useState("");
  const actor = session?.user?.id || session?.user?.email;
  React.useEffect(() => {
    let active = true;
    setMode(null);
    setError("");
    if (session?.authenticated)
      getMarketWallet()
        .then((wallet) => {
          if (active) setMode(wallet.mode);
        })
        .catch((cause) => {
          if (active)
            setError(cause instanceof Error ? cause.message : String(cause));
        });
    return () => {
      active = false;
    };
  }, [actor, session?.authenticated]);
  if (error)
    return (
      <p role="alert" className="p-5 text-sm text-destructive">
        {error}
      </p>
    );
  return mode ? (
    <ProviderFinancePanel key={actor} walletMode={mode} />
  ) : (
    <p className="p-5 text-sm text-muted-foreground">{t("common.loading")}</p>
  );
}
