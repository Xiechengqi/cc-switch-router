"use client";

import { Button } from "@heroui/react";
import Link from "next/link";
import { CircleAlert, CircleCheck, Loader2, RefreshCw } from "lucide-react";
import * as React from "react";

import { useLocaleText } from "@/components/i18n/locale-provider";
import { getAccountPaymentProfile, getMarketBillingDashboard } from "@/lib/api";
import {
  DASHBOARD_ACCOUNT_EARNINGS_PATH,
  DASHBOARD_ACCOUNT_PROVIDER_SETTINGS_PATH,
  DASHBOARD_ACCOUNT_WALLET_PATH,
} from "@/lib/dashboard-nav";
import { MARKET_CURRENCY } from "@/lib/market-money";
import { getMarketProviderFinance, getMarketWallet } from "@/lib/market-wallet-api";

export type PaidOfferReadiness = {
  status: "idle" | "loading" | "loaded" | "error";
  paymentReady: boolean;
  settlementReady: boolean;
  rail: "legacy" | "wallet" | null;
  walletSetup: "ready" | "provider" | "destination" | "cooling" | null;
};

const INITIAL_READINESS: PaidOfferReadiness = {
  status: "idle",
  paymentReady: false,
  settlementReady: false,
  rail: null,
  walletSetup: null,
};

export function usePaidOfferReadiness(enabled: boolean) {
  const [readiness, setReadiness] = React.useState<PaidOfferReadiness>(INITIAL_READINESS);
  const requestRef = React.useRef(0);

  const reload = React.useCallback(async () => {
    const request = ++requestRef.current;
    setReadiness((current) => ({ ...current, status: "loading" }));
    try {
      const wallet = await getMarketWallet();
      if (wallet.mode === "enabled") {
        const finance = await getMarketProviderFinance();
        if (request !== requestRef.current) return;
        const walletSetup = !finance.isProvider ? "provider"
          : !finance.destination ? "destination"
            : !Number.isFinite(Date.parse(finance.destination.usableAt))
              || Date.parse(finance.destination.usableAt) > Date.now() ? "cooling" : "ready";
        setReadiness({
          status: "loaded",
          rail: "wallet",
          walletSetup,
          paymentReady: true,
          settlementReady: walletSetup === "ready",
        });
        return;
      }
      const [payment, billing] = await Promise.all([
        getAccountPaymentProfile(), getMarketBillingDashboard(),
      ]);
      if (request !== requestRef.current) return;
      setReadiness({
        status: "loaded",
        paymentReady: payment.methods.length > 0,
        settlementReady: billing.supplierProfiles.some(
          (profile) => profile.currency === MARKET_CURRENCY,
        ),
        rail: "legacy",
        walletSetup: null,
      });
    } catch {
      if (request !== requestRef.current) return;
      setReadiness({
        status: "error",
        paymentReady: false,
        settlementReady: false,
        rail: null,
        walletSetup: null,
      });
    }
  }, []);

  React.useEffect(() => {
    if (!enabled) {
      requestRef.current += 1;
      setReadiness(INITIAL_READINESS);
      return;
    }
    void reload();
    return () => {
      requestRef.current += 1;
    };
  }, [enabled, reload]);

  return {
    ...readiness,
    blocked:
      enabled &&
      (readiness.status !== "loaded" ||
        !readiness.paymentReady ||
        !readiness.settlementReady),
    reload,
  };
}

function ReadinessLine({ ready, children }: { ready: boolean; children: React.ReactNode }) {
  const Icon = ready ? CircleCheck : CircleAlert;
  return (
    <span className="inline-flex min-w-0 items-center gap-1.5">
      <Icon className={ready ? "h-4 w-4 shrink-0 text-emerald-600" : "h-4 w-4 shrink-0 text-amber-700"} />
      <span className="min-w-0">{children}</span>
    </span>
  );
}

export function PaidOfferReadinessNotice({
  readiness,
}: {
  readiness: ReturnType<typeof usePaidOfferReadiness>;
}) {
  const { t } = useLocaleText();

  if (readiness.status === "idle") return null;
  if (readiness.status === "loading") {
    return (
      <div role="status" className="flex items-center gap-2 rounded-md border border-slate-200 bg-slate-50 px-3 py-2.5 text-sm text-slate-600">
        <Loader2 className="h-4 w-4 shrink-0 animate-spin" />
        {t("shareMarket.paidReadiness.checking")}
      </div>
    );
  }
  if (readiness.status === "error") {
    return (
      <div role="alert" className="flex flex-wrap items-center gap-3 rounded-md border border-rose-200 bg-rose-50 px-3 py-2.5 text-sm text-rose-800">
        <CircleAlert className="h-4 w-4 shrink-0" />
        <span className="min-w-[12rem] flex-1">{t("shareMarket.paidReadiness.failed")}</span>
        <Button className="whitespace-nowrap" size="sm" variant="outline" onClick={() => void readiness.reload()}>
          <RefreshCw className="h-4 w-4" />
          {t("common.retry")}
        </Button>
      </div>
    );
  }

  const ready = readiness.paymentReady && readiness.settlementReady;
  return (
    <div role={ready ? "status" : "alert"} className={ready
      ? "flex items-center gap-2 rounded-md border border-emerald-200 bg-emerald-50 px-3 py-2.5 text-sm text-emerald-800"
      : "grid gap-2 rounded-md border border-amber-200 bg-amber-50 px-3 py-2.5 text-sm text-amber-950"
    }>
      {ready ? (
        <>
          <CircleCheck className="h-4 w-4 shrink-0" />
          {t("shareMarket.paidReadiness.ready")}
        </>
      ) : readiness.rail === "wallet" ? (
        <>
          <strong>{t("shareMarket.paidReadiness.title")}</strong>
          <ReadinessLine ready={false}>
            {t(`shareMarket.paidReadiness.wallet.${readiness.walletSetup || "destination"}`)}
          </ReadinessLine>
          <Link className="whitespace-nowrap font-medium underline underline-offset-2" href={DASHBOARD_ACCOUNT_WALLET_PATH}>
            {t("shareMarket.paidReadiness.goWallet")}
          </Link>
        </>
      ) : (
        <>
          <strong>{t("shareMarket.paidReadiness.title")}</strong>
          <div className="grid gap-1.5 sm:grid-cols-2">
            <ReadinessLine ready={readiness.paymentReady}>
              {readiness.paymentReady
                ? t("shareMarket.paidReadiness.paymentReady")
                : t("shareMarket.paidReadiness.paymentMissing")}
            </ReadinessLine>
            <ReadinessLine ready={readiness.settlementReady}>
              {readiness.settlementReady
                ? t("shareMarket.paidReadiness.settlementReady")
                : t("shareMarket.paidReadiness.settlementMissing")}
            </ReadinessLine>
          </div>
          <div className="flex flex-wrap gap-x-4 gap-y-1">
            {!readiness.paymentReady ? (
              <Link className="whitespace-nowrap font-medium underline underline-offset-2" href={DASHBOARD_ACCOUNT_PROVIDER_SETTINGS_PATH}>
                {t("shareMarket.paidReadiness.goPayments")}
              </Link>
            ) : null}
            {!readiness.settlementReady ? (
              <Link className="whitespace-nowrap font-medium underline underline-offset-2" href={DASHBOARD_ACCOUNT_EARNINGS_PATH}>
                {t("shareMarket.paidReadiness.goSettlement")}
              </Link>
            ) : null}
          </div>
        </>
      )}
    </div>
  );
}
