"use client";

import * as React from "react";
import { Button, Modal, toast } from "@heroui/react";
import { Loader2 } from "lucide-react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { getAccountPaymentProfile, getMarketBillingDashboard, updateClientMarketHostOffer } from "@/lib/api";
import { DASHBOARD_ACCOUNT_BILLING_PATH, DASHBOARD_ACCOUNT_PAYMENTS_PATH } from "@/lib/dashboard-nav";
import type { ClientMarketHost } from "@/lib/types";
import { isPaymentProfileRequiredError, parseHostOffer } from "@/components/dashboard/client-market/host-utils";

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
  const [price, setPrice] = React.useState(host.dailyRateMinor ? (host.dailyRateMinor / 100).toFixed(2) : "");
  const [currency, setCurrency] = React.useState<"CNY" | "USD">(host.currency === "CNY" ? "CNY" : "USD");
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState("");
  const [paymentReady, setPaymentReady] = React.useState<boolean | null>(null);
  const [billingCurrencies, setBillingCurrencies] = React.useState<string[] | null>(null);

  React.useEffect(() => {
    if (!open) return;
    setPrice(host.dailyRateMinor ? (host.dailyRateMinor / 100).toFixed(2) : "");
    setCurrency(host.currency === "CNY" ? "CNY" : "USD");
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
  }, [host.currency, host.dailyRateMinor, open]);

  const save = async () => {
    let offer: ReturnType<typeof parseHostOffer>;
    try {
      offer = parseHostOffer(price, t, currency);
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
      return;
    }
    if (offer.dailyRateMinor && (paymentReady === false || !billingCurrencies?.includes(currency))) {
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
            {paymentReady === false || (billingCurrencies && !billingCurrencies.includes(currency)) ? (
              <div className="grid gap-2 rounded-lg border border-amber-200 bg-amber-50 px-3 py-3 text-sm text-amber-950">
                <p>{t("clientMarket.offerRequiresBilling")}</p>
                <div className="flex flex-wrap gap-3">
                  <a href={DASHBOARD_ACCOUNT_BILLING_PATH} className="font-medium text-foreground underline underline-offset-2">{t("clientMarket.goToBilling")}</a>
                  <a href={DASHBOARD_ACCOUNT_PAYMENTS_PATH} className="font-medium text-foreground underline underline-offset-2">{t("clientMarket.goToAccountPayment")}</a>
                </div>
              </div>
            ) : null}
            <div className="grid gap-3 sm:grid-cols-[minmax(0,1fr)_7rem]">
              <label className="grid gap-1 text-sm">
                <span className="text-muted-foreground">{t("clientMarket.dailyPrice")}</span>
                <input
                  value={price}
                  onChange={(event) => setPrice(event.target.value)}
                  placeholder={t("clientMarket.free")}
                  inputMode="decimal"
                  className="h-10 rounded-md border px-3"
                />
              </label>
              <label className="grid gap-1 text-sm">
                <span className="text-muted-foreground">{t("clientMarket.currency")}</span>
                <select
                  value={currency}
                  onChange={(event) => setCurrency(event.target.value === "CNY" ? "CNY" : "USD")}
                  className="h-10 rounded-md border px-2"
                >
                  <option value="CNY">CNY</option>
                  <option value="USD">USD</option>
                </select>
              </label>
            </div>
            <p className="text-xs text-muted-foreground">{t("clientMarket.makeFreeHint")}</p>
            {error ? <p className="text-sm text-rose-600">{error}</p> : null}
          </Modal.Body>
          <Modal.Footer>
            <Button variant="ghost" isDisabled={busy} onClick={() => onOpenChange(false)}>{t("common.cancel")}</Button>
            <Button variant="primary" isDisabled={busy || paymentReady === null || billingCurrencies === null} onClick={() => void save()}>
              {busy ? <Loader2 className="h-4 w-4 animate-spin" /> : null}
              {t("common.save")}
            </Button>
          </Modal.Footer>
        </Modal.Dialog>
      </Modal.Container>
    </Modal.Backdrop>
  );
}
