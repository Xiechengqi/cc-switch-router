"use client";

import * as React from "react";
import { Button } from "@heroui/react";
import { useAuth } from "@/components/auth/auth-provider";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { PlatformWalletPanel } from "@/components/dashboard/platform-wallet-panel";
import { getMarketWallet, type MarketWallet } from "@/lib/market-wallet-api";

export function AccountWalletPage() {
  const { t } = useLocaleText();
  const { session, loading } = useAuth();
  const [wallet, setWallet] = React.useState<{ actorId: string; value: MarketWallet } | null>(null);
  const [error, setError] = React.useState<{ actorId: string; message: string } | null>(null);
  const actorId = session?.user?.id || session?.user?.email || "authenticated";

  const reload = React.useCallback(async () => {
    setError(null);
    try {
      setWallet({ actorId, value: await getMarketWallet() });
    } catch (cause) {
      setError({ actorId, message: cause instanceof Error ? cause.message : String(cause) });
    }
  }, [actorId]);

  React.useEffect(() => {
    if (!session?.authenticated) return;
    let active = true;
    getMarketWallet().then((view) => {
      if (active) { setWallet({ actorId, value: view }); setError(null); }
    }).catch((cause) => {
      if (active) setError({ actorId, message: cause instanceof Error ? cause.message : String(cause) });
    });
    return () => { active = false; };
  }, [actorId, session?.authenticated]);

  if (loading || !session?.authenticated) {
    return <div className="p-6 text-sm text-muted-foreground">{t("common.loadingSession")}</div>;
  }
  if (error?.actorId === actorId) {
    return <div className="grid gap-3 p-6"><p role="alert" className="text-sm text-destructive">{error.message}</p><Button variant="outline" onPress={() => void reload()}>{t("common.retry")}</Button></div>;
  }
  if (wallet?.actorId !== actorId) return <div className="p-6 text-sm text-muted-foreground">{t("common.loading")}</div>;
  return <React.Fragment key={actorId}>
    <PlatformWalletPanel initialWallet={wallet.value} />
  </React.Fragment>;
}
