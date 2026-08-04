"use client";

import * as React from "react";
import { Button, Chip } from "@heroui/react";
import { MessageCircle } from "lucide-react";
import { useClientChat } from "@/components/chat/client-chat";
import { ClientMarketRentalBanner } from "@/components/dashboard/client-market-rental-banner";
import { CountryFlag } from "@/components/common/country-flag";
import { ReleaseRentalAction } from "@/components/dashboard/client-market/release-rental-action";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { recoveryStateLabelKey } from "@/components/dashboard/client-market/host-utils";
import type { ClientMarketHost, ClientMarketRental } from "@/lib/types";

/**
 * "My rentals" — the renter's side of the Client Market.
 *
 * This used to have no home. A renter's allocated Clients appeared as rows inside
 * the Provider-oriented host table, distinguished only by an `isClientOwner` flag,
 * with rental state bolted on as a subrow. That forced one table to serve two unrelated
 * jobs, which is why its filters and subrows were reworked repeatedly.
 *
 * Note `/v1/client-market/my-rentals` returns rows where the viewer is *either* the
 * renter or the Provider, so `isClientOwner` must be filtered here.
 */
export function MyRentalsPanel({
  rentals,
  hostsByInstallation,
  onChanged,
}: {
  rentals: ClientMarketRental[];
  hostsByInstallation: Map<string, ClientMarketHost>;
  onChanged: () => Promise<void> | void;
}) {
  const { t } = useLocaleText();
  const { openChat, unreadByInstallation } = useClientChat();

  const activeRentals = React.useMemo(
    () =>
      rentals
        .filter((rental) => rental.isClientOwner && rental.status !== "released")
        .sort((left, right) => left.installationId.localeCompare(right.installationId)),
    [rentals],
  );

  if (!activeRentals.length) {
    return (
      <div className="grid justify-items-center gap-2 rounded-xl border border-dashed border-border bg-card/40 px-4 py-12 text-center text-sm text-muted-foreground">
        <span>{t("clientMarket.rentals.empty")}</span>
      </div>
    );
  }

  return (
    <div className="grid gap-3">
      {activeRentals.map((rental) => {
        const host = hostsByInstallation.get(rental.installationId);
        const subdomain = host?.clientSubdomain;
        const chatUnread = unreadByInstallation.get(rental.installationId) || 0;
        return (
          <section
            key={rental.installationId}
            className="grid gap-3 rounded-xl border border-border bg-card p-4 shadow-sm sm:p-5"
          >
            <div className="flex min-w-0 flex-wrap items-center gap-2">
              {host?.countryCode ? (
                <CountryFlag code={host.countryCode} className="h-3.5 w-5 rounded-sm object-cover" />
              ) : null}
              <strong className="truncate text-sm">
                {subdomain || host?.hostname || rental.installationId.slice(0, 12)}
              </strong>
              {host ? (
                <Chip size="sm" variant="tertiary">
                  {host.status}
                </Chip>
              ) : null}
              {host?.recovery ? (
                <Chip size="sm" variant="soft">
                  {t(recoveryStateLabelKey(host.recovery.state))}
                </Chip>
              ) : null}
              <span className="min-w-0 flex-1 truncate text-xs text-muted-foreground">
                {t("clientMarket.rentals.provider", { owner: rental.hostOwnerEmail })}
              </span>
              <span className="relative inline-flex" title={t("clientMarket.openGroupChat")}>
                <Button
                  variant="ghost"
                  size="sm"
                  isIconOnly
                  className="h-8 w-8 min-w-8 shrink-0"
                  onClick={() => void openChat(rental.installationId)}
                  aria-label={t("clientMarket.openGroupChat")}
                >
                  <MessageCircle className="h-4 w-4" />
                </Button>
                {chatUnread > 0 ? (
                  <span className="pointer-events-none absolute -right-1 -top-1 min-w-4 rounded-full bg-red-600 px-1 text-center text-[9px] font-semibold leading-4 text-white">
                    {chatUnread > 99 ? "99+" : chatUnread}
                  </span>
                ) : null}
              </span>
              {/* Keep mounted through `releasing` so the same instance can finish polling
                  (and reattach after refresh). Retries for release_failed live in the banner. */}
              {rental.status !== "release_failed" ? (
                <ReleaseRentalAction rental={rental} onChanged={onChanged} />
              ) : null}
            </div>
            <ClientMarketRentalBanner
              rental={rental}
              onChanged={onChanged}
              resumeRelease={false}
            />
          </section>
        );
      })}
    </div>
  );
}
