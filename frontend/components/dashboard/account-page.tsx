"use client";

// Compatibility export for old imports. Never revive Provider-specific payment UI.
import { Suspense } from "react";
import { AccountLegacyRedirect } from "@/components/dashboard/account-legacy-redirect";
export function AccountPaymentsPanel() {
  return (
    <Suspense fallback={null}>
      <AccountLegacyRedirect target="/account/provider-settings/" />
    </Suspense>
  );
}
