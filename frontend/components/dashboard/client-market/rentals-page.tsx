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
 * The renter's surface, owning its own data.
 *
 * It previously lived as a tab inside the Provider-oriented Client Market page and
 * borrowed that page's polling. Renting and providing are different jobs done at
 * different cadences, so this loads independently — a renter never pays for the
 * host table's state, and vice versa.
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

  const load = React.useCallback(
    async (options?: { silent?: boolean }) => {
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
    },
    [],
  );

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

  /** `my-billing` also returns rows where the viewer is the Provider; the panel
   *  filters those out, but the count shown here must match what it renders. */
  const myRentals = React.useMemo(
    () => Array.from(billingByInstallation.values()).filter((billing) => billing.isClientOwner),
    [billingByInstallation],
  );

  const hostsByInstallation = React.useMemo(() => {
    const map = new Map<string, ClientMarketHost>();
    for (const host of hosts) {
      if (host.installationId) map.set(host.installationId, host);
    }
    return map;
  }, [hosts]);

  return (
    <main className="mx-auto grid min-w-0 w-[calc(100%-2rem)] max-w-7xl grid-cols-[minmax(0,1fr)] gap-5 pb-10">
      <div className="flex flex-wrap items-baseline justify-between gap-2">
        <h1 className="text-sm font-semibold text-foreground">{t("clientMarket.tabMyRentals")}</h1>
        {myRentals.length ? (
          <span className="text-xs text-muted-foreground">
            {t("clientMarket.rentals.count", { count: myRentals.length })}
          </span>
        ) : null}
      </div>

      {!authed ? (
        <p className="text-sm text-muted-foreground">{t("clientMarket.rentals.loginRequired")}</p>
      ) : loading ? (
        <div className="flex items-center gap-2 text-sm text-muted-foreground">
          <Loader2 className="h-4 w-4 animate-spin" />…
        </div>
      ) : error ? (
        <p className="text-sm text-rose-600">{error}</p>
      ) : (
        <MyRentalsPanel
          billings={myRentals}
          hostsByInstallation={hostsByInstallation}
          onChanged={silentRefresh}
        />
      )}
    </main>
  );
}
