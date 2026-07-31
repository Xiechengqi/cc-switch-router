"use client";

import * as React from "react";
import { Button, Modal, toast } from "@heroui/react";
import { Loader2 } from "lucide-react";
import { SegmentedControl } from "@/components/common/segmented-control";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { getAccountPaymentProfile, getMarketBillingDashboard, updateClientMarketHostOffer } from "@/lib/api";
import { DASHBOARD_ACCOUNT_BILLING_PATH, DASHBOARD_ACCOUNT_PAYMENTS_PATH } from "@/lib/dashboard-nav";
import { MARKET_CURRENCY } from "@/lib/market-money";
import type { ClientMarketHost } from "@/lib/types";
import {
  isPaymentProfileRequiredError,
  parseFreeDurationDays,
  parseHostOffer,
} from "@/components/dashboard/client-market/host-utils";

export function HostOfferDialog({
  host,
  open,
  onOpenChange,
  onSaved,
}: {
  host: ClientMarketHost;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onSaved: () => void;
}) {
  const { t } = useLocaleText();
  const [pricing, setPricing] = React.useState<"free" | "paid">(
    host.dailyRateMinor ? "paid" : "free",
  );
  const [price, setPrice] = React.useState(host.dailyRateMinor ? (host.dailyRateMinor / 100).toFixed(2) : "");
  const [freeDurationMode, setFreeDurationMode] = React.useState<"fixed" | "permanent">(
    host.freeDurationDays == null ? "permanent" : "fixed",
  );
  const [freeDurationDays, setFreeDurationDays] = React.useState(
    String(host.freeDurationDays ?? 1),
  );
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState("");
  const [paymentReady, setPaymentReady] = React.useState<boolean | null>(null);
  const [billingCurrencies, setBillingCurrencies] = React.useState<string[] | null>(null);

  React.useEffect(() => {
    if (!open) return;
    setPricing(host.dailyRateMinor ? "paid" : "free");
    setPrice(host.dailyRateMinor ? (host.dailyRateMinor / 100).toFixed(2) : "");
    setFreeDurationMode(host.freeDurationDays == null ? "permanent" : "fixed");
    setFreeDurationDays(String(host.freeDurationDays ?? 1));
    setError("");
    setPaymentReady(null);
    setBillingCurrencies(null);
    let cancelled = false;
    void Promise.all([getAccountPaymentProfile(), getMarketBillingDashboard()])
      .then(([profile, billing]) => {
        if (!cancelled) {
          setPaymentReady(profile.methods.length > 0);
          setBillingCurrencies(billing.supplierProfiles.map((item) => item.currency));
        }
      })
      .catch(() => {
        if (!cancelled) {
          setPaymentReady(false);
          setBillingCurrencies([]);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [host.dailyRateMinor, host.freeDurationDays, open]);

  const save = async () => {
    let offer: {
      dailyRateMinor?: number;
      currency?: string;
      freeDurationDays?: number;
    };
    try {
      offer = pricing === "paid"
        ? parseHostOffer(price, t)
        : {
            freeDurationDays:
              freeDurationMode === "fixed"
                ? parseFreeDurationDays(freeDurationDays, t)
                : undefined,
          };
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
      return;
    }
    if (pricing === "paid" && (!offer.dailyRateMinor || paymentReady === false || !billingCurrencies?.includes(MARKET_CURRENCY))) {
      if (!offer.dailyRateMinor) {
        setError(t("clientMarket.offerInvalid"));
        return;
      }
      setError(t("clientMarket.offerRequiresBilling"));
      return;
    }
    setBusy(true);
    setError("");
    try {
      await updateClientMarketHostOffer(host.id, offer);
      toast.success(t("clientMarket.offerUpdated"));
      onSaved();
      onOpenChange(false);
    } catch (reason) {
      const message = reason instanceof Error ? reason.message : String(reason);
      setError(isPaymentProfileRequiredError(message) ? t("clientMarket.offerRequiresPayment") : message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal.Backdrop isOpen={open} onOpenChange={(next) => !busy && onOpenChange(next)}>
      <Modal.Container placement="center">
        <Modal.Dialog className="light w-[min(460px,calc(100vw-2rem))] max-w-none !bg-white !text-slate-900">
          <Modal.Header><Modal.Heading>{t("clientMarket.editOffer")}</Modal.Heading></Modal.Header>
          <Modal.Body className="grid gap-4">
            <p className="text-sm text-muted-foreground">{t("clientMarket.editOfferHint")}</p>
            {pricing === "paid" && (paymentReady === false || (billingCurrencies && !billingCurrencies.includes(MARKET_CURRENCY))) ? (
              <div className="grid gap-2 rounded-lg border border-amber-200 bg-amber-50 px-3 py-3 text-sm text-amber-950">
                <p>{t("clientMarket.offerRequiresBilling")}</p>
                <div className="flex flex-wrap gap-3">
                  <a href={DASHBOARD_ACCOUNT_BILLING_PATH} className="font-medium text-foreground underline underline-offset-2">{t("clientMarket.goToBilling")}</a>
                  <a href={DASHBOARD_ACCOUNT_PAYMENTS_PATH} className="font-medium text-foreground underline underline-offset-2">{t("clientMarket.goToAccountPayment")}</a>
                </div>
              </div>
            ) : null}
            <SegmentedControl
              value={pricing}
              onChange={setPricing}
              ariaLabel={t("clientMarket.currentOffer")}
              size="md"
              fullWidth
              items={[
                { id: "free", label: t("clientMarket.free") },
                { id: "paid", label: t("clientMarket.paid") },
              ]}
            />
            {pricing === "paid" ? (
              <div className="grid gap-3 sm:grid-cols-[minmax(0,1fr)_7rem]">
                <label className="grid gap-1 text-sm">
                  <span className="text-muted-foreground">{t("clientMarket.dailyPrice")}</span>
                  <input value={price} onChange={(event) => setPrice(event.target.value)} inputMode="decimal" className="h-10 rounded-md border px-3" />
                </label>
                <label className="grid gap-1 text-sm">
                  <span className="text-muted-foreground">{t("clientMarket.currency")}</span>
                  <span className="flex h-10 items-center rounded-md border bg-slate-50 px-3 font-medium">{MARKET_CURRENCY}</span>
                </label>
              </div>
            ) : (
              <div className="grid gap-3">
                <SegmentedControl
                  value={freeDurationMode}
                  onChange={setFreeDurationMode}
                  ariaLabel={t("clientMarket.freeDuration.days")}
                  size="md"
                  fullWidth
                  items={[
                    { id: "fixed", label: t("clientMarket.freeDuration.fixed") },
                    { id: "permanent", label: t("clientMarket.freeDuration.permanent") },
                  ]}
                />
                {freeDurationMode === "fixed" ? (
                  <label className="grid gap-1 text-sm">
                    <span className="text-muted-foreground">{t("clientMarket.freeDuration.days")}</span>
                    <input type="number" min={1} max={365} step={1} value={freeDurationDays} onChange={(event) => setFreeDurationDays(event.target.value)} className="h-10 rounded-md border px-3" />
                  </label>
                ) : null}
              </div>
            )}
            {error ? <p className="text-sm text-rose-600">{error}</p> : null}
          </Modal.Body>
          <Modal.Footer>
            <Button variant="ghost" isDisabled={busy} onClick={() => onOpenChange(false)}>{t("common.cancel")}</Button>
            <Button variant="primary" isDisabled={busy || (pricing === "paid" && (paymentReady === null || billingCurrencies === null))} onClick={() => void save()}>
              {busy ? <Loader2 className="h-4 w-4 animate-spin" /> : null}
              {t("common.save")}
            </Button>
          </Modal.Footer>
        </Modal.Dialog>
      </Modal.Container>
    </Modal.Backdrop>
  );
}
