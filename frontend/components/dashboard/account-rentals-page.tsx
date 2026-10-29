"use client";

import { Receipt } from "lucide-react";
import { AccountMarketRecords } from "@/components/dashboard/account-market-records";
import { AccountSectionHeader } from "@/components/dashboard/account-section";
import { useLocaleText } from "@/components/i18n/locale-provider";

/** Everything the account rents, from both markets. What it rents out lives in the Provider center. */
export function AccountRentalsPage() {
  const { t } = useLocaleText();
  return (
    <div className="grid min-w-0 gap-4">
      <AccountSectionHeader icon={Receipt} title={t("account.nav.myRentals")} hint={t("account.rentals.hint")} />
      <AccountMarketRecords role="user" />
    </div>
  );
}
