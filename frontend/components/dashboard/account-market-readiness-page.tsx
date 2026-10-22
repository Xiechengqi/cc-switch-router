"use client";

// Compatibility export for old imports. Never revive Provider-specific payment UI.
import { Suspense } from "react";
import { AccountLegacyRedirect } from "@/components/dashboard/account-legacy-redirect";
import { DASHBOARD_ACCOUNT_PROVIDER_SETTINGS_PATH } from "@/lib/dashboard-nav";
export function AccountMarketReadinessPage() {
  return (
    <Suspense fallback={null}>
      <AccountLegacyRedirect
        target={DASHBOARD_ACCOUNT_PROVIDER_SETTINGS_PATH}
      />
    </Suspense>
  );
}
