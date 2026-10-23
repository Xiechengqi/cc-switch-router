import { AccountContactPanel } from "@/components/dashboard/account-contact-panel";
import { AccountMarketAccessPage } from "@/components/dashboard/account-market-access-page";
import { ProviderFinancePanel } from "@/components/dashboard/provider-finance-panel";
export default function Page() {
  return (
    <div className="grid min-w-0 gap-6">
      <ProviderFinancePanel />
      <AccountContactPanel />
      <AccountMarketAccessPage />
    </div>
  );
}
