"use client";

import * as React from "react";
import { useAuth } from "@/components/auth/auth-provider";
import { PageSkeleton } from "@/components/common/page-skeleton";
import { AccountSectionError } from "@/components/dashboard/account-section";
import { PlatformWalletPanel } from "@/components/dashboard/platform-wallet-panel";
import { rememberedView } from "@/lib/cached-resource";
import { getMarketWallet, type MarketWallet } from "@/lib/market-wallet-api";

const walletViewKey = (actorId: string) => `wallet:${actorId}`;

export function AccountWalletPage() {
  const { session } = useAuth();
  const actorId = session?.user?.id || session?.user?.email || "authenticated";
  return <WalletLoader key={actorId} actorId={actorId} />;
}

/**
 * The panel reloads balance and history as soon as it mounts and remembers
 * the latest balance. So a return visit paints that balance at once with no
 * extra request; only the first visit fetches the balance here before the
 * panel can start.
 */
function WalletLoader({ actorId }: { actorId: string }) {
  const viewKey = walletViewKey(actorId);
  const [wallet, setWallet] = React.useState<MarketWallet | undefined>(() => rememberedView<MarketWallet>(viewKey));
  const [error, setError] = React.useState("");
  const [attempt, setAttempt] = React.useState(0);
  const needsFetch = wallet === undefined;

  React.useEffect(() => {
    if (!needsFetch) return;
    let active = true;
    setError("");
    getMarketWallet().then((view) => {
      if (active) setWallet(view);
    }).catch((cause) => {
      if (active) setError(cause instanceof Error ? cause.message : String(cause));
    });
    return () => { active = false; };
  }, [attempt, needsFetch]);

  if (wallet) return <PlatformWalletPanel initialWallet={wallet} rememberKey={viewKey} />;
  if (error) return <AccountSectionError message={error} onRetry={() => setAttempt((value) => value + 1)} />;
  return <PageSkeleton inset />;
}
