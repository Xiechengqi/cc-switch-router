"use client";

import { useLocaleText } from "@/components/i18n/locale-provider";

/** Compatibility export only: consumption balances cannot be withdrawn. */
export function MarketWalletRefundPanel() {
  const { t } = useLocaleText();
  return <p className="text-sm text-muted-foreground">{t("walletPolicy.disclosure")}</p>;
}
