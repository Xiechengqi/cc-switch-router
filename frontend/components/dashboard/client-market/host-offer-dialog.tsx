"use client";

import * as React from "react";
import { Button, Modal, toast } from "@heroui/react";
import { Loader2, RefreshCw } from "lucide-react";
import { SegmentedControl } from "@/components/common/segmented-control";
import { usePaidOfferReadiness } from "@/components/dashboard/share-market/paid-offer-readiness";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { updateClientMarketHostOffer } from "@/lib/api";
import { DASHBOARD_ACCOUNT_BILLING_PATH, DASHBOARD_ACCOUNT_PAYMENTS_PATH } from "@/lib/dashboard-nav";
import { MARKET_CURRENCY } from "@/lib/market-money";
import type { ClientMarketHost } from "@/lib/types";
import {
  hostPaidOfferPrerequisiteError,
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
  const paidReadiness = usePaidOfferReadiness(open && pricing === "paid");
  const paidOfferLoading = paidReadiness.status === "idle" ||
    paidReadiness.status === "loading";
  const paidOfferReady = paidReadiness.status === "loaded" &&
    paidReadiness.paymentReady &&
    paidReadiness.settlementReady;

  React.useEffect(() => {
    if (!open) return;
    setPricing(host.dailyRateMinor ? "paid" : "free");
    setPrice(host.dailyRateMinor ? (host.dailyRateMinor / 100).toFixed(2) : "");
    setFreeDurationMode(host.freeDurationDays == null ? "permanent" : "fixed");
    setFreeDurationDays(String(host.freeDurationDays ?? 1));
    setError("");
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
    if (pricing === "paid" && (!offer.dailyRateMinor || !paidOfferReady)) {
      if (!offer.dailyRateMinor) {
        setError(t("clientMarket.offerInvalid"));
        return;
      }
      setError(t(paidReadiness.status === "error"
        ? "clientMarket.offerSetupFailed"
        : paidOfferLoading
          ? "clientMarket.offerSetupChecking"
          : "clientMarket.offerRequiresBilling"));
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
      setError(
        hostPaidOfferPrerequisiteError(reason, t) ||
          t("clientMarket.offerUpdateFailed"),
      );
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal.Backdrop isOpen={open} onOpenChange={(next) => !busy && onOpenChange(next)}>
      <Modal.Container placement="center">
        <Modal.Dialog className="light w-[min(460px,calc(100vw-2rem))] max-w-none !bg-white !text-slate-900">
          <Modal.Header><Modal.Heading>{t("clientMarket.editOffer")}</Modal.Heading></Modal.Header>
          <Modal.Body className="grid max-h-[75vh] gap-4 overflow-y-auto">
            <p className="text-sm text-muted-foreground">{t("clientMarket.editOfferHint")}</p>
            {pricing === "paid" && paidOfferLoading ? (
              <div role="status" className="flex items-center gap-2 rounded-lg border border-slate-200 bg-slate-50 px-3 py-3 text-sm text-slate-600">
                <Loader2 className="h-4 w-4 shrink-0 animate-spin" />
                {t("clientMarket.offerSetupChecking")}
              </div>
            ) : null}
            {pricing === "paid" && paidReadiness.status === "error" ? (
              <div role="alert" className="flex flex-wrap items-center gap-3 rounded-lg border border-rose-200 bg-rose-50 px-3 py-3 text-sm text-rose-800">
                <span className="min-w-[12rem] flex-1">{t("clientMarket.offerSetupFailed")}</span>
                <Button className="whitespace-nowrap" size="sm" variant="outline" onClick={() => void paidReadiness.reload()}>
                  <RefreshCw className="h-4 w-4" />
                  {t("common.retry")}
                </Button>
              </div>
            ) : null}
            {pricing === "paid" && paidReadiness.status === "loaded" && !paidOfferReady ? (
              <div role="alert" className="grid gap-2 rounded-lg border border-amber-200 bg-amber-50 px-3 py-3 text-sm text-amber-950">
                <p>{t("clientMarket.offerRequiresBilling")}</p>
                <div className="flex flex-wrap gap-3">
                  {!paidReadiness.settlementReady ? <a href={`${DASHBOARD_ACCOUNT_BILLING_PATH}?tab=receivables`} className="whitespace-nowrap font-medium text-foreground underline underline-offset-2">{t("clientMarket.goToBilling")}</a> : null}
                  {!paidReadiness.paymentReady ? <a href={DASHBOARD_ACCOUNT_PAYMENTS_PATH} className="whitespace-nowrap font-medium text-foreground underline underline-offset-2">{t("clientMarket.goToAccountPayment")}</a> : null}
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
            <Button variant="primary" isDisabled={busy || (pricing === "paid" && !paidOfferReady)} onClick={() => void save()}>
              {busy ? <Loader2 className="h-4 w-4 animate-spin" /> : null}
              {t("common.save")}
            </Button>
          </Modal.Footer>
        </Modal.Dialog>
      </Modal.Container>
    </Modal.Backdrop>
  );
}
