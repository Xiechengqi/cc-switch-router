"use client";

import * as React from "react";
import { Button, Modal, toast } from "@heroui/react";
import { Loader2 } from "lucide-react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { getAccountPaymentProfile, updateClientMarketHostOffer } from "@/lib/api";
import { DASHBOARD_ACCOUNT_PATH } from "@/lib/dashboard-nav";
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
  const [price, setPrice] = React.useState(host.priceCents ? (host.priceCents / 100).toFixed(2) : "");
  const [period, setPeriod] = React.useState(host.rentalPeriodDays ? String(host.rentalPeriodDays) : "");
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState("");
  const [paymentReady, setPaymentReady] = React.useState<boolean | null>(null);

  React.useEffect(() => {
    if (!open) return;
    setPrice(host.priceCents ? (host.priceCents / 100).toFixed(2) : "");
    setPeriod(host.rentalPeriodDays ? String(host.rentalPeriodDays) : "");
    setError("");
    setPaymentReady(null);
    let cancelled = false;
    void getAccountPaymentProfile()
      .then((profile) => {
        if (!cancelled) setPaymentReady(profile.methods.length > 0);
      })
      .catch(() => {
        if (!cancelled) setPaymentReady(false);
      });
    return () => {
      cancelled = true;
    };
  }, [host.priceCents, host.rentalPeriodDays, open]);

  const save = async () => {
    let offer: ReturnType<typeof parseHostOffer>;
    try {
      offer = parseHostOffer(price, period, t);
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
      return;
    }
    if (offer.priceCents && paymentReady === false) {
      setError(t("clientMarket.offerRequiresPayment"));
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
            {paymentReady === false ? (
              <div className="grid gap-2 rounded-lg border border-amber-200 bg-amber-50 px-3 py-3 text-sm text-amber-950">
                <p>{t("clientMarket.offerRequiresPayment")}</p>
                <a
                  href={DASHBOARD_ACCOUNT_PATH}
                  className="inline-flex w-fit items-center font-medium text-foreground underline underline-offset-2"
                >
                  {t("clientMarket.goToAccountPayment")}
                </a>
              </div>
            ) : null}
            <div className="grid gap-3 sm:grid-cols-2">
              <label className="grid gap-1 text-sm">
                <span className="text-muted-foreground">{t("clientMarket.priceUsd")}</span>
                <input
                  value={price}
                  onChange={(event) => setPrice(event.target.value)}
                  placeholder={t("clientMarket.free")}
                  inputMode="decimal"
                  className="h-10 rounded-md border px-3"
                />
              </label>
              <label className="grid gap-1 text-sm">
                <span className="text-muted-foreground">{t("clientMarket.periodDays")}</span>
                <input
                  value={period}
                  onChange={(event) => setPeriod(event.target.value)}
                  placeholder={t("clientMarket.forever")}
                  inputMode="numeric"
                  className="h-10 rounded-md border px-3"
                />
              </label>
            </div>
            <p className="text-xs text-muted-foreground">{t("clientMarket.makeFreeHint")}</p>
            {error ? <p className="text-sm text-rose-600">{error}</p> : null}
          </Modal.Body>
          <Modal.Footer>
            <Button variant="ghost" isDisabled={busy} onClick={() => onOpenChange(false)}>{t("common.cancel")}</Button>
            <Button variant="primary" isDisabled={busy || paymentReady === null} onClick={() => void save()}>
              {busy ? <Loader2 className="h-4 w-4 animate-spin" /> : null}
              {t("common.save")}
            </Button>
          </Modal.Footer>
        </Modal.Dialog>
      </Modal.Container>
    </Modal.Backdrop>
  );
}
