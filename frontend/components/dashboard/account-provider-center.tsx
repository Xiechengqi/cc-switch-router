"use client";

/* Hallmark · component: Provider center · genre: modern-minimal · theme: existing Router system
 * states: default · focus · selected · badge · signed-out
 * Three independent jobs — income, buyer access, contact — as tabs instead of
 * one long scroll. The tab lives in the URL so links can open the right one.
 */

import * as React from "react";
import { useSearchParams } from "next/navigation";
import { useAuth } from "@/components/auth/auth-provider";
import { SegmentedControl } from "@/components/common/segmented-control";
import { AccountContactPanel } from "@/components/dashboard/account-contact-panel";
import { AccountMarketAccessPage } from "@/components/dashboard/account-market-access-page";
import { ProviderFinancePanel } from "@/components/dashboard/provider-finance-panel";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { getMarketProviderFinance } from "@/lib/market-wallet-api";
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
  const [hasIncome, setHasIncome] = React.useState<boolean | null>(null);
  const [defaultError, setDefaultError] = React.useState(false);
  const requestedTab = searchParams.get("tab");
  const explicitTab = PROVIDER_CENTER_TABS.includes(requestedTab as ProviderCenterTab) || hash === "#income";
  const tab = providerCenterTab(requestedTab, hash, hasIncome === true);

  React.useEffect(() => {
    const read = () => setHash(window.location.hash);
    read();
    window.addEventListener("hashchange", read);
    return () => window.removeEventListener("hashchange", read);
  }, []);

  React.useEffect(() => {
    if (!actor) return;
    let active = true;
    void getMarketProviderFinance().then((finance) => {
      if (active) setHasIncome(finance.hasFinanceActivity);
    }).catch(() => { if (active) { setHasIncome(false); setDefaultError(true); } });
    return () => { active = false; };
  }, [actor]);

  const setTab = (next: ProviderCenterTab) => {
    setHash("");
    replaceCurrentSearch((params) => params.set("tab", next), { dropHash: true });
  };

  const labels: Record<ProviderCenterTab, string> = {
    income: t("providerCenter.tab.income"),
    access: t("providerCenter.tab.access"),
    contact: t("providerCenter.tab.contact"),
  };

  if (!actor) return <p className="text-sm text-muted-foreground">{t("account.signInRequired")}</p>;
  if (hasIncome === null && !explicitTab) return <p role="status" className="text-sm text-muted-foreground">{t("common.loading")}</p>;

  return (
    <div className="grid min-w-0 gap-4">
      <div>
        <h2 className="text-base font-semibold text-foreground">{t("account.nav.providerSettings")}</h2>
        <p className="mt-0.5 text-sm text-muted-foreground">{t("providerCenter.hint")}</p>
      </div>
      <SegmentedControl
        value={tab}
        onChange={setTab}
        ariaLabel={t("providerCenter.sections")}
        size="md"
        className="w-full max-w-md"
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
        {defaultError ? <p role="status" className="mb-3 text-sm text-muted-foreground">{t("providerCenter.defaultUnavailable")}</p> : null}
        {tab === "income" ? <ProviderFinancePanel /> : tab === "access" ? <AccountMarketAccessPage onPendingChange={setPendingRequests} /> : <AccountContactPanel />}
      </div>
    </div>
  );
}
