"use client";

/* Hallmark · component: Provider center · genre: modern-minimal · theme: existing Router system
 * states: default · focus · selected · badge · signed-out
 * Everything a Provider runs — what is rented out, its income, buyer access,
 * contact — as tabs instead of one long scroll. The tab lives in the URL so
 * links can open the right one; without one the page opens at once on what
 * is rented out, which every Provider has, instead of waiting to learn
 * whether there is income yet.
 */

import * as React from "react";
import { useSearchParams } from "next/navigation";
import { Store } from "lucide-react";
import { useAuth } from "@/components/auth/auth-provider";
import { SegmentedControl } from "@/components/common/segmented-control";
import { AccountContactPanel } from "@/components/dashboard/account-contact-panel";
import { AccountMarketAccessPage } from "@/components/dashboard/account-market-access-page";
import { AccountMarketRecords } from "@/components/dashboard/account-market-records";
import { AccountSectionHeader } from "@/components/dashboard/account-section";
import { ProviderFinancePanel } from "@/components/dashboard/provider-finance-panel";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { PROVIDER_CENTER_TABS, providerCenterTab, type ProviderCenterTab } from "@/lib/dashboard-nav";
import { replaceCurrentSearch } from "@/lib/url-search";
import { useMarketAccessInboxSummary } from "@/lib/use-market-access-inbox-summary";

export function AccountProviderCenter() {
  const { session } = useAuth();
  const actor = session?.authenticated ? session.user?.id || session.user?.email : undefined;
  return <ProviderCenterWorkspace key={actor || "anonymous"} actor={actor} />;
}

function ProviderCenterWorkspace({ actor }: { actor?: string }) {
  const { t } = useLocaleText();
  const searchParams = useSearchParams();
  const [hash, setHash] = React.useState("");
  const { pendingRequests, setPendingRequests } = useMarketAccessInboxSummary(actor);
  const tab = providerCenterTab(searchParams.get("tab"), hash);

  React.useEffect(() => {
    const read = () => setHash(window.location.hash);
    read();
    window.addEventListener("hashchange", read);
    return () => window.removeEventListener("hashchange", read);
  }, []);

  const setTab = (next: ProviderCenterTab) => {
    setHash("");
    replaceCurrentSearch((params) => {
      params.set("tab", next);
      // Market filter and focus belong to the listings tab.
      if (next !== "listings") {
        params.delete("market");
        params.delete("focus");
      }
    }, { dropHash: true });
  };

  const labels: Record<ProviderCenterTab, string> = {
    listings: t("providerCenter.tab.listings"),
    income: t("providerCenter.tab.income"),
    access: t("providerCenter.tab.access"),
    contact: t("providerCenter.tab.contact"),
  };

  if (!actor) return null;

  return (
    <div className="grid min-w-0 gap-4">
      <AccountSectionHeader icon={Store} title={t("account.nav.providerSettings")} hint={t("providerCenter.hint")} />
      <SegmentedControl
        value={tab}
        onChange={setTab}
        ariaLabel={t("providerCenter.sections")}
        size="md"
        className="w-full max-w-xl"
        fullWidth
        items={PROVIDER_CENTER_TABS.map((id) => ({
          id,
          ariaLabel: id === "access" && pendingRequests > 0
            ? `${labels[id]} · ${t("providerCenter.pendingRequests", { count: pendingRequests })}`
            : labels[id],
          label: (
            <span className="inline-flex items-center gap-1.5">
              {labels[id]}
              {id === "access" && pendingRequests > 0 ? (
                <span aria-hidden className="inline-flex min-w-5 items-center justify-center rounded-full bg-rose-100 px-1.5 text-[10px] font-semibold tabular-nums text-rose-700">
                  {pendingRequests > 99 ? "99+" : pendingRequests}
                </span>
              ) : null}
            </span>
          ),
        }))}
      />
      <div role="tabpanel" aria-label={labels[tab]} className="min-w-0">
        {tab === "listings" ? (
          <div className="grid min-w-0 gap-4">
            <p className="text-sm text-muted-foreground">{t("account.listings.hint")}</p>
            <AccountMarketRecords role="provider" />
          </div>
        ) : tab === "income" ? <ProviderFinancePanel /> : tab === "access" ? <AccountMarketAccessPage onPendingChange={setPendingRequests} /> : <AccountContactPanel />}
      </div>
    </div>
  );
}
