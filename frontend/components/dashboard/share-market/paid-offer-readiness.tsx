"use client";

import { Button } from "@heroui/react";
import Link from "next/link";
import { CircleAlert, CircleCheck, Loader2, RefreshCw } from "lucide-react";
import * as React from "react";

import { useLocaleText } from "@/components/i18n/locale-provider";
import { DASHBOARD_ACCOUNT_PROVIDER_SETTINGS_PATH } from "@/lib/dashboard-nav";
import { getMarketProviderFinance } from "@/lib/market-wallet-api";

export type PaidOfferReadiness = {
  status: "idle" | "loading" | "loaded" | "error";
  providerReady: boolean;
};

const INITIAL_READINESS: PaidOfferReadiness = {
  status: "idle",
  providerReady: false,
};

export function usePaidOfferReadiness(enabled: boolean) {
  const [readiness, setReadiness] = React.useState<PaidOfferReadiness>(INITIAL_READINESS);
  const requestRef = React.useRef(0);

  const reload = React.useCallback(async () => {
    const request = ++requestRef.current;
    setReadiness((current) => ({ ...current, status: "loading" }));
    try {
      // Paid publication uses the Provider identity, not official recharge
      // availability or retired direct-payment/settlement profiles.
      const finance = await getMarketProviderFinance();
      if (typeof finance.canPublishPaidOffers !== "boolean") throw new Error("Invalid Provider readiness");
      if (request !== requestRef.current) return;
      setReadiness({
        status: "loaded",
        providerReady: finance.canPublishPaidOffers,
      });
    } catch {
      if (request !== requestRef.current) return;
      setReadiness({
        status: "error",
        providerReady: false,
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
        !readiness.providerReady),
    reload,
  };
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

  const ready = readiness.providerReady;
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
      ) : (
        <>
          <strong>{t("shareMarket.paidReadiness.title")}</strong>
          <span>{t("shareMarket.paidReadiness.wallet.provider")}</span>
          <Link className="whitespace-nowrap font-medium underline underline-offset-2" href={DASHBOARD_ACCOUNT_PROVIDER_SETTINGS_PATH}>
            {t("shareMarket.paidReadiness.goProvider")}
          </Link>
        </>
      )}
    </div>
  );
}
