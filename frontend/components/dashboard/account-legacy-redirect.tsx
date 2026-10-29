"use client";
import { useEffect } from "react";
import { useRouter } from "next/navigation";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { DASHBOARD_ACCOUNT_PATH, legacyAccountHref } from "@/lib/dashboard-nav";

/** Sends a retired account address, with its role, market and focus, to where that view lives now. */
export function AccountLegacyRedirect() {
  const router = useRouter();
  const { t } = useLocaleText();
  useEffect(() => {
    router.replace(legacyAccountHref(window.location.pathname, window.location.search) || DASHBOARD_ACCOUNT_PATH);
  }, [router]);
  return (
    <p role="status" className="py-6 text-sm text-muted-foreground">
      {t("common.loading")}
    </p>
  );
}
