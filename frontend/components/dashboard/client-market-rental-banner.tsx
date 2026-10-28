"use client";

import { trialHours, walletLifecycleKey } from "@/lib/client-checkout";
import { Clock3, HandCoins, Loader2 } from "lucide-react";
import {
  FinalizeFailedRentalAction,
  ReleaseRentalAction,
} from "@/components/dashboard/client-market/release-rental-action";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { DASHBOARD_ACCOUNT_WALLET_PATH } from "@/lib/dashboard-nav";
import { clientMarketCleanupReasonKey, clientMarketRentalHasBanner } from "@/lib/client-market-rental-state";
import { cleanupFailureGuidanceKey } from "@/components/dashboard/client-market/host-utils";
import type { ClientMarketRental } from "@/lib/types";
import { ClientRentalSchedule } from "@/components/dashboard/client-market/client-rental-schedule";

/** Rental and wallet lifecycle are separate from Client connection health. */
export function ClientMarketRentalBanner({
  rental,
  onChanged,
  compact = false,
  resumeRelease = true,
  readOnly = false,
  manageHref,
  showManageLink = true,
  showSchedule = true,
  summaryOnly = false,
}: {
  rental?: ClientMarketRental;
  onChanged: () => Promise<void> | void;
  compact?: boolean;
  resumeRelease?: boolean;
  readOnly?: boolean;
  manageHref?: string;
  showManageLink?: boolean;
  /** Free-duration schedule is verbose; list rows hide it and surface it in the client drawer instead. */
  showSchedule?: boolean;
  /** Dense status cells keep trial/exception states, not normal dates or links. */
  summaryOnly?: boolean;
}) {
  const { locale, t } = useLocaleText();
  if (!clientMarketRentalHasBanner(rental)) return null;
  if (!rental) return null;
  const nextProbe = rental.nextCleanupProbeAt
    ? new Intl.DateTimeFormat(locale, { dateStyle: "short", timeStyle: "short" }).format(new Date(rental.nextCleanupProbeAt))
    : null;
  const probeStopped = !nextProbe && (rental.cleanupProbeAttempts ?? 0) >= 5;

  if (rental.status === "released" && rental.hostCleanupPending) {
    return (
      <span className="inline-flex flex-wrap items-center gap-1.5 text-[11px] text-amber-800" data-no-row-drawer onClick={(event) => event.stopPropagation()}>
        {rental.activeCleanupJobId ? <Loader2 className="h-3 w-3 animate-spin" /> : null}
        <span>{t(rental.activeCleanupJobId ? "clientMarket.release.hostCleaning" : "clientMarket.release.hostPending")}</span>
        {!rental.activeCleanupJobId && rental.cleanupFailureKind ? <span>{t(clientMarketCleanupReasonKey(rental.cleanupFailureKind))}</span> : null}
        {nextProbe ? <span>{t("clientMarket.release.nextProbe", { time: nextProbe })}</span> : null}
        {probeStopped ? <span>{t("clientMarket.release.probeStopped")}</span> : null}
        {rental.walletContract ? <span>{t(walletLifecycleKey(rental.walletContract), { hours: trialHours(rental.walletContract.trialSecondsRemaining, locale), date: "—" })}</span> : null}
      </span>
    );
  }

  if (rental.status === "releasing") {
    return (
      <span className="inline-flex flex-wrap items-center gap-1.5 text-[11px] text-muted-foreground" data-no-row-drawer onClick={(event) => event.stopPropagation()}>
        <Loader2 className="h-3 w-3 animate-spin" />
        {t("clientMarket.release.releasing")}
        {!readOnly && resumeRelease ? <ReleaseRentalAction rental={rental} onChanged={onChanged} /> : null}
        {!readOnly ? <FinalizeFailedRentalAction rental={rental} onChanged={onChanged} /> : null}
      </span>
    );
  }

  if (rental.status === "release_failed") {
    return (
      <span className="inline-flex flex-wrap items-center gap-1.5" data-no-row-drawer onClick={(event) => event.stopPropagation()}>
        <span className="text-[11px] text-rose-700">
          {rental.cleanupFailureKind
            ? t(cleanupFailureGuidanceKey(rental.cleanupFailureKind))
            : t("clientMarket.release.failedHint")}
        </span>
        {nextProbe ? <span className="text-[11px] text-muted-foreground">{t("clientMarket.release.nextProbe", { time: nextProbe })}</span> : null}
        {probeStopped ? <span className="text-[11px] text-muted-foreground">{t("clientMarket.release.probeStopped")}</span> : null}
        {!readOnly ? (
          <ReleaseRentalAction rental={rental} onChanged={onChanged} label={t("clientMarket.release.retry")} className="h-6 px-2 text-[11px] text-rose-700" />
        ) : showManageLink && manageHref ? (
          <a href={manageHref} className="text-[11px] font-medium text-accent hover:underline">{t("clientMarket.release.manageInMarket")}</a>
        ) : null}
        {!readOnly ? <FinalizeFailedRentalAction rental={rental} onChanged={onChanged} /> : null}
      </span>
    );
  }

  if (rental.walletContract) {
    return <ClientRentalSchedule contract={rental.walletContract} compact={compact} summaryOnly={summaryOnly}
      showSchedule={showSchedule} showManageLink={showManageLink} manageHref={manageHref} />;
  }

  if (rental.pricingModel === "free" || (rental.dailyRateMinor == null && rental.cyclePriceMinor == null)) {
    if (summaryOnly || !showSchedule) return null;
    const activated = rental.activatedAt
      ? new Date(rental.activatedAt).toLocaleString(locale)
      : null;
    const expires = rental.expiresAt ? new Date(rental.expiresAt) : null;
    const remainingMs = expires ? expires.getTime() - Date.now() : null;
    const remaining = remainingMs == null
      ? t("clientMarket.freeDuration.permanent")
      : remainingMs <= 0
        ? t("clientMarket.freeDuration.expired")
        : remainingMs < 48 * 60 * 60 * 1000
          ? t("clientMarket.freeDuration.remainingHours", { count: Math.ceil(remainingMs / 3_600_000) })
          : t("clientMarket.freeDuration.remainingDays", { count: Math.ceil(remainingMs / 86_400_000) });
    const detail = activated
      ? expires
        ? `${t("clientMarket.freeDuration.activated")}: ${activated} · ${t("clientMarket.freeDuration.expires")}: ${expires.toLocaleString(locale)} · ${remaining}`
        : `${t("clientMarket.freeDuration.activated")}: ${activated} · ${remaining}`
      : t("clientMarket.freeDuration.pendingActivation");
    return (
      <span
        className={`inline-flex items-center gap-1 text-muted-foreground ${compact ? "text-[11px]" : "text-xs"}`}
        title={detail}
      >
        <Clock3 className="h-3 w-3" />
        {compact
          ? activated
            ? remaining
            : t("clientMarket.freeDuration.pendingActivation")
          : detail}
      </span>
    );
  }
  if (summaryOnly || !showManageLink) return null;
  return (
    <a
      href={DASHBOARD_ACCOUNT_WALLET_PATH}
      className={`inline-flex items-center gap-1 rounded-full border border-slate-200 bg-white font-medium text-slate-700 hover:border-slate-300 hover:bg-slate-50 ${compact ? "h-6 px-2 text-[11px]" : "h-7 px-2.5 text-xs"}`}
      data-no-row-drawer
      onClick={(event) => event.stopPropagation()}
    >
      <HandCoins className="h-3 w-3" />
      {t("account.nav.wallet")}
    </a>
  );
}
