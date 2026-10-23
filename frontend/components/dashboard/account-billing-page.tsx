"use client";

// Compatibility export for old imports. Never revive Provider-specific payment UI.
import { Suspense } from "react";
import { AccountLegacyRedirect } from "@/components/dashboard/account-legacy-redirect";
import { DASHBOARD_ACCOUNT_WALLET_PATH } from "@/lib/dashboard-nav";
export function AccountBillingPage() {
  return (
    <Suspense fallback={null}>
      <AccountLegacyRedirect target={DASHBOARD_ACCOUNT_WALLET_PATH} />
    </Suspense>
  );
}
