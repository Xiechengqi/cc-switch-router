"use client";

import * as React from "react";
import { useSearchParams } from "next/navigation";
import { SegmentedControl } from "@/components/common/segmented-control";
import { AccountClientSection } from "@/components/dashboard/account-client-section";
import { AccountShareSection, type AccountMarketRole } from "@/components/dashboard/account-share-section";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { ACCOUNT_MARKET_FILTERS, accountMarketFilter, type AccountMarketFilter } from "@/lib/dashboard-nav";
import { replaceCurrentSearch } from "@/lib/url-search";

const SECTION_TITLES = {
  user: { share: "account.rentals.shareSection", client: "account.rentals.clientSection" },
  provider: { share: "account.listings.shareSection", client: "account.listings.clientSection" },
} as const;

/**
 * One role's records across both markets: Share seats, then Client leases.
 * `?market=` narrows to one market; `?focus=` only applies together with it,
 * because a Share focus pages through history until it finds the row.
 */
export function AccountMarketRecords({ role }: { role: AccountMarketRole }) {
  const { t } = useLocaleText();
  const searchParams = useSearchParams();
  const market = accountMarketFilter(searchParams.get("market"));
  const focus = searchParams.get("focus");

  const setMarket = (next: AccountMarketFilter) => replaceCurrentSearch((params) => {
    if (next === "all") params.delete("market");
    else params.set("market", next);
    params.delete("focus");
  });

  return (
    <div className="grid min-w-0 gap-6">
      <SegmentedControl
        value={market}
        onChange={setMarket}
        ariaLabel={t("account.market.filter")}
        size="sm"
        className="w-full max-w-sm"
        fullWidth
        items={ACCOUNT_MARKET_FILTERS.map((id) => ({ id, label: t(`account.market.filter.${id}`) }))}
      />
      {(["share", "client"] as const).filter((item) => market === "all" || market === item).map((item) => (
        <section key={item} aria-labelledby={`account-${role}-${item}`} className="grid min-w-0 gap-3">
          <h3 id={`account-${role}-${item}`} className="text-sm font-semibold text-foreground">{t(SECTION_TITLES[role][item])}</h3>
          {item === "share"
            ? <AccountShareSection role={role} focusId={market === "share" ? focus : null} />
            : <AccountClientSection role={role} focusKey={market === "client" ? focus : null} />}
        </section>
      ))}
    </div>
  );
}
