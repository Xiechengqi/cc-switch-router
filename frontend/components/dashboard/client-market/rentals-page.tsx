"use client";

import * as React from "react";
import { Loader2 } from "lucide-react";
import { useAuth } from "@/components/auth/auth-provider";
import { MyRentalsPanel } from "@/components/dashboard/client-market/my-rentals-panel";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { getClientMarketHosts, getMyClientMarketBilling } from "@/lib/api";
import { mergeBillingMap, mergeHosts } from "@/lib/client-market-refresh";
import { CLIENT_MARKET_POLL_MS } from "@/components/dashboard/client-market/host-utils";
import type { ClientMarketBilling, ClientMarketHost } from "@/lib/types";

/**
 * Renter surface for Account → Client 租用.
 * Loads independently from Provider-oriented Client Market host polling.
 */
export function RentalsPage() {
  const { t } = useLocaleText();
  const { session } = useAuth();
  const authed = !!session?.authenticated;

  const [hosts, setHosts] = React.useState<ClientMarketHost[]>([]);
  const [billingByInstallation, setBillingByInstallation] = React.useState<
    Map<string, ClientMarketBilling>
  >(new Map());
  const [loading, setLoading] = React.useState(true);
  const [error, setError] = React.useState("");
  const refreshAbortRef = React.useRef<AbortController | null>(null);

  const load = React.useCallback(async (options?: { silent?: boolean }) => {
    const silent = options?.silent === true;
    if (silent && refreshAbortRef.current) return;
    if (!silent) refreshAbortRef.current?.abort();
    const controller = new AbortController();
    refreshAbortRef.current = controller;
    if (!silent) {
      setLoading(true);
      setError("");
    }
    try {
      const [nextHosts, billing] = await Promise.all([
        getClientMarketHosts(undefined, controller.signal),
        getMyClientMarketBilling(controller.signal),
      ]);
      if (controller.signal.aborted) return;
      setHosts((prev) => mergeHosts(prev, nextHosts));
      setBillingByInstallation((prev) => mergeBillingMap(prev, billing));
    } catch (err) {
      if (controller.signal.aborted) return;
      if (!silent) setError(err instanceof Error ? err.message : String(err));
    } finally {
      if (refreshAbortRef.current === controller) refreshAbortRef.current = null;
      if (!silent) setLoading(false);
    }
  }, []);

  const silentRefresh = React.useCallback(() => load({ silent: true }), [load]);

  React.useEffect(() => {
    if (!authed) {
      setLoading(false);
      return;
    }
    void load();
  }, [authed, load]);

  React.useEffect(() => {
    if (!authed) return;
    const timer = window.setInterval(() => void load({ silent: true }), CLIENT_MARKET_POLL_MS);
    return () => window.clearInterval(timer);
  }, [authed, load]);

  const myRentals = React.useMemo(
    () =>
      Array.from(billingByInstallation.values()).filter(
        (billing) => billing.isClientOwner && billing.status !== "released",
      ),
    [billingByInstallation],
  );

  /** Badge counts only in-rent subscriptions; releasing / release_failed stay visible in the list. */
  const activeRentalCount = React.useMemo(
    () =>
      myRentals.filter(
        (billing) => billing.status === "active" || billing.status === "payment_due",
      ).length,
    [myRentals],
  );

  const hostsByInstallation = React.useMemo(() => {
    const map = new Map<string, ClientMarketHost>();
    for (const host of hosts) {
      if (host.installationId) map.set(host.installationId, host);
    }
    return map;
  }, [hosts]);

  if (!authed) {
    return <p className="py-6 text-sm text-muted-foreground">{t("clientMarket.rentals.loginRequired")}</p>;
  }

  if (loading) {
    return (
      <div className="flex items-center gap-2 py-6 text-sm text-muted-foreground">
        <Loader2 className="h-4 w-4 animate-spin" />…
      </div>
    );
  }

  if (error) {
    return <p className="py-6 text-sm text-rose-600">{error}</p>;
  }

  return (
    <div className="grid min-w-0 grid-cols-[minmax(0,1fr)] gap-4">
      <div className="flex flex-wrap items-baseline justify-between gap-2">
        <div>
          <h2 className="text-base font-semibold text-foreground">{t("account.nav.rentals")}</h2>
          <p className="mt-0.5 text-sm text-muted-foreground">{t("account.rentalsHint")}</p>
        </div>
        {activeRentalCount ? (
          <span className="text-xs text-muted-foreground">
            {t("clientMarket.rentals.count", { count: activeRentalCount })}
          </span>
        ) : null}
      </div>
      <MyRentalsPanel
        billings={myRentals}
        hostsByInstallation={hostsByInstallation}
        onChanged={silentRefresh}
      />
    </div>
  );
}
