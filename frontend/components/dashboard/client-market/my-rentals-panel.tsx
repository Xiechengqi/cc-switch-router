"use client";

import * as React from "react";
import { Chip } from "@heroui/react";
import { ClientMarketBillingBanner } from "@/components/dashboard/client-market-billing-banner";
import { CountryFlag } from "@/components/common/country-flag";
import { useLocaleText } from "@/components/i18n/locale-provider";
import type { ClientMarketBilling, ClientMarketHost } from "@/lib/types";

/**
 * "My rentals" — the renter's side of the Client Market.
 *
 * This used to have no home. A renter's allocated Clients appeared as rows inside
 * the Provider-oriented host table, distinguished only by an `isClientOwner` flag,
 * with billing bolted on as a subrow. That forced one table to serve two unrelated
 * jobs, which is why its filters and subrows were reworked repeatedly.
 *
 * Note `/v1/client-market/my-billing` returns rows where the viewer is *either* the
 * renter or the Provider, so `isClientOwner` must be filtered here.
 */
export function MyRentalsPanel({
  billings,
  hostsByInstallation,
  onChanged,
}: {
  billings: ClientMarketBilling[];
  hostsByInstallation: Map<string, ClientMarketHost>;
  onChanged: () => Promise<void> | void;
}) {
  const { t } = useLocaleText();

  const rentals = React.useMemo(
    () =>
      billings
        .filter((billing) => billing.isClientOwner && billing.status !== "released")
        .sort((left, right) => left.installationId.localeCompare(right.installationId)),
    [billings],
  );

  if (!rentals.length) {
    return (
      <div className="grid justify-items-center gap-2 rounded-lg border border-dashed border-border px-4 py-10 text-center text-sm text-muted-foreground">
        <span>{t("clientMarket.rentals.empty")}</span>
      </div>
    );
  }

  return (
    <div className="grid gap-3">
      {rentals.map((billing) => {
        const host = hostsByInstallation.get(billing.installationId);
        const subdomain = host?.clientSubdomain;
        return (
          <section
            key={billing.installationId}
            className="grid gap-3 rounded-xl border border-border bg-card p-4 shadow-sm"
          >
            <div className="flex min-w-0 flex-wrap items-center gap-2">
              {host?.countryCode ? (
                <CountryFlag code={host.countryCode} className="h-3.5 w-5 rounded-sm object-cover" />
              ) : null}
              <strong className="truncate text-sm">
                {subdomain || host?.hostname || billing.installationId.slice(0, 12)}
              </strong>
              {host ? (
                <Chip size="sm" variant="tertiary">
                  {host.status}
                </Chip>
              ) : null}
              <span className="min-w-0 flex-1 truncate text-xs text-muted-foreground">
                {t("clientMarket.rentals.provider", { owner: billing.hostOwnerEmail })}
              </span>
            </div>
            <ClientMarketBillingBanner billing={billing} onChanged={onChanged} showPayButton />
          </section>
        );
      })}
    </div>
  );
}
