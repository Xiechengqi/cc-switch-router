"use client";

import { CalendarClock } from "lucide-react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { trialHours, walletLifecycleKey } from "@/lib/client-checkout";
import { DASHBOARD_ACCOUNT_WALLET_PATH } from "@/lib/dashboard-nav";
import type { WalletLifecycle } from "@/lib/types";
import { ClientNextTrialNotice } from "./client-next-trial-notice";

/** Separate normal cycle details from the trial/exception hints in dense rows. */
export function ClientRentalSchedule({ contract, compact = false, summaryOnly = false,
  showSchedule = true, showManageLink = true, manageHref }: {
  contract: WalletLifecycle;
  compact?: boolean;
  summaryOnly?: boolean;
  showSchedule?: boolean;
  showManageLink?: boolean;
  manageHref?: string;
}) {
  const { locale, t } = useLocaleText();
  if (summaryOnly && contract.status === "active") return null;
  const date = contract.periodEnd && Number.isFinite(Date.parse(contract.periodEnd))
    ? new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short" }).format(new Date(contract.periodEnd)) : "—";
  return <span className={`inline-flex min-w-0 flex-wrap items-center gap-1.5 text-muted-foreground ${compact ? "text-[11px]" : "text-xs"}`}>
    <CalendarClock className="h-3 w-3 shrink-0" aria-hidden="true" />
    <span>{t(walletLifecycleKey(contract), { hours: trialHours(contract.trialSecondsRemaining, locale), date })}</span>
    {!summaryOnly && !compact && showSchedule ? <ClientNextTrialNotice trial={contract.nextRentalTrial} className="basis-full" /> : null}
    {!summaryOnly && showManageLink ? <a href={manageHref || DASHBOARD_ACCOUNT_WALLET_PATH} className="font-medium text-primary hover:underline">{t(manageHref ? "clientMarket.release.manageInMarket" : "account.nav.wallet")}</a> : null}
  </span>;
}
