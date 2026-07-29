"use client";

import * as React from "react";
import { Button, Chip, Modal, toast } from "@heroui/react";
import { Loader2, Mail, WalletCards } from "lucide-react";
import { BillingUrgencyChip } from "@/components/dashboard/billing-urgency-chip";
import { ConfirmAlertDialog } from "@/components/common/confirm-alert-dialog";
import { ReleaseRentalAction } from "@/components/dashboard/client-market/release-rental-action";
import { AuthenticatedImage } from "@/components/common/authenticated-image";
import { PaymentMethodIcons } from "@/components/common/payment-method-icons";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { declareClientMarketPayment } from "@/lib/api";
import { billingUrgencyTier, formatBillingCountdown } from "@/lib/billing-urgency";
import type { ClientMarketBilling, ClientMarketPaymentMethod } from "@/lib/types";

function offerLabel(billing: ClientMarketBilling, locale: string) {
  if (!billing.priceCents || !billing.rentalPeriodDays) return locale.startsWith("zh") ? "免费 / 永久" : "Free / forever";
  const amount = new Intl.NumberFormat(locale, { style: "currency", currency: "USD" }).format(billing.priceCents / 100);
  return locale.startsWith("zh") ? `${amount} / ${billing.rentalPeriodDays} 天` : `${amount} / ${billing.rentalPeriodDays} days`;
}

function PaymentMethod({ method }: { method: ClientMarketPaymentMethod }) {
  const { t } = useLocaleText();
  const label = method.kind === "alipay"
    ? t("billing.payment.alipay")
    : method.kind === "wechat"
      ? t("billing.payment.wechat")
      : method.kind === "binance"
        ? t("billing.payment.binance")
        : method.kind === "crypto"
          ? t("billing.payment.crypto")
          : method.kind === "custom"
            ? t("billing.payment.custom")
            : method.kind;
  return (
    <section className="grid min-w-0 grid-cols-[minmax(0,1fr)] gap-2 border-b border-border py-3 last:border-b-0">
      <div className="flex min-w-0 flex-wrap items-center gap-2">
        <PaymentMethodIcons kinds={[method.kind]} />
        <strong className="text-sm">{label}</strong>
        {method.token ? <Chip size="sm" variant="soft">{method.token}</Chip> : null}
        {method.chain ? <Chip size="sm" variant="soft">{method.chain.toUpperCase()}</Chip> : null}
      </div>
      {method.account ? <div className="min-w-0 break-words text-sm"><span className="text-muted-foreground">{t("billing.account")}: </span><span className="font-medium">{method.account}</span></div> : null}
      {method.address ? <code className="break-all rounded-md bg-slate-50 px-3 py-2 text-xs">{method.address}</code> : null}
      {method.instructions ? <p className="whitespace-pre-wrap break-words text-sm leading-6 text-slate-700">{method.instructions}</p> : null}
      {method.assetUrl ? <AuthenticatedImage src={method.assetUrl} alt={t("billing.qrAlt", { method: label })} className="h-40 w-40 rounded-md border bg-white object-contain p-1" /> : null}
    </section>
  );
}

/** Inline billing urgency + payment/release modals for Client Market rows.
 *  Clients board uses `readOnly` — pay/release live only on Client Market. */
export function ClientMarketBillingBanner({
  billing,
  onChanged,
  compact = false,
  showPayButton = true,
  /** When true (default), mount release resume UI for `releasing`. */
  resumeRelease = true,
  /** Urgency cue only — no pay modal, no release / retry actions. */
  readOnly = false,
  manageHref,
}: {
  billing?: ClientMarketBilling;
  onChanged: () => Promise<void> | void;
  compact?: boolean;
  showPayButton?: boolean;
  resumeRelease?: boolean;
  readOnly?: boolean;
  /** Optional deep-link shown next to read-only urgency (e.g. Client Market「我的」). */
  manageHref?: string;
}) {
  const { locale, t } = useLocaleText();
  const [paymentOpen, setPaymentOpen] = React.useState(false);
  const [confirmPayment, setConfirmPayment] = React.useState(false);
  const [busy, setBusy] = React.useState(false);

  if (!billing || !billing.isClientOwner || billing.status === "released") return null;

  const manageLink = manageHref ? (
    <a
      href={manageHref}
      className="inline-flex h-6 items-center rounded-md px-1.5 text-[11px] font-medium text-accent hover:underline"
      data-no-row-drawer
      onClick={(event) => event.stopPropagation()}
    >
      {t("billing.manageInMarket")}
    </a>
  ) : null;

  const declarePaid = async () => {
    if (!billing.openInvoiceId) return;
    setBusy(true);
    try {
      await declareClientMarketPayment(
        billing.installationId,
        billing.openInvoiceId,
        billing.offerRevision,
        billing.paymentProfileUpdatedAt,
        billing.priceCents,
      );
      toast.success(t("billing.declaredToast"));
      setConfirmPayment(false);
      setPaymentOpen(false);
      await onChanged();
    } catch (reason) {
      setConfirmPayment(false);
      toast.danger(reason instanceof Error ? reason.message : String(reason));
      await onChanged();
    } finally {
      setBusy(false);
    }
  };

  if (billing.status === "releasing") {
    return (
      <span
        className="inline-flex flex-wrap items-center gap-1.5 text-[11px] text-muted-foreground"
        data-no-row-drawer
        onClick={(event) => event.stopPropagation()}
      >
        <Loader2 className="h-3 w-3 animate-spin" />
        {t("billing.releasing")}
        {!readOnly && resumeRelease ? <ReleaseRentalAction billing={billing} onChanged={onChanged} /> : null}
        {readOnly ? manageLink : null}
      </span>
    );
  }

  const methods = billing.paymentMethods || [];
  const tier = billingUrgencyTier(billing);
  const dueCountdown = formatBillingCountdown(
    billing.paymentDeadline || billing.currentPeriodEnd,
    locale,
  );
  const canPay = billing.status === "payment_due" || tier === "urgent";

  if (billing.status === "release_failed") {
    return (
      <span className="inline-flex flex-wrap items-center gap-1.5" data-no-row-drawer onClick={(event) => event.stopPropagation()}>
        <span className="text-[11px] text-rose-700">{t("billing.releaseFailed")}</span>
        {readOnly ? (
          manageLink
        ) : (
          <ReleaseRentalAction
            billing={billing}
            onChanged={onChanged}
            label={t("billing.retryRelease")}
            className="h-6 px-2 text-[11px] text-rose-700"
          />
        )}
      </span>
    );
  }

  if (!tier || tier === "silent") return null;

  if (readOnly) {
    return (
      <span className="inline-flex flex-wrap items-center gap-1.5" data-no-row-drawer onClick={(event) => event.stopPropagation()}>
        <BillingUrgencyChip billing={billing} compact={compact} showPayButton={false} />
        {manageLink}
      </span>
    );
  }

  return (
    <>
      <BillingUrgencyChip
        billing={billing}
        compact={compact}
        showPayButton={showPayButton && canPay}
        onPay={() => setPaymentOpen(true)}
      />

      <Modal.Backdrop isOpen={paymentOpen} onOpenChange={(next) => !busy && setPaymentOpen(next)}>
        <Modal.Container placement="center">
          <Modal.Dialog className="light min-w-0 w-[min(620px,calc(100vw-2rem))] max-w-none overflow-hidden !bg-white !text-slate-900">
            <Modal.Header><Modal.Heading>{t("billing.payProvider")}</Modal.Heading></Modal.Header>
            <Modal.Body className="grid min-w-0 max-h-[70vh] grid-cols-[minmax(0,1fr)] gap-4 overflow-y-auto">
              <div className="grid min-w-0 grid-cols-[minmax(0,1fr)] gap-2 rounded-md border border-border bg-slate-50 p-3 text-sm">
                <div className="grid min-w-0 gap-1 sm:grid-cols-[auto_minmax(0,1fr)] sm:items-start sm:gap-3"><span className="text-muted-foreground">{t("billing.provider")}</span><strong className="min-w-0 break-all sm:text-right">{billing.hostOwnerEmail}</strong></div>
                <div className="grid min-w-0 gap-1 sm:grid-cols-[auto_minmax(0,1fr)] sm:items-start sm:gap-3"><span className="text-muted-foreground">{t("billing.currentOffer")}</span><strong className="min-w-0 break-words sm:text-right">{offerLabel(billing, locale)}</strong></div>
                <div className="grid min-w-0 gap-1 sm:grid-cols-[auto_minmax(0,1fr)] sm:items-start sm:gap-3"><span className="text-muted-foreground">{t("billing.declareBefore")}</span><strong className="min-w-0 break-words sm:text-right">{billing.paymentDeadline ? new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short" }).format(new Date(billing.paymentDeadline)) : billing.currentPeriodEnd ? new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short" }).format(new Date(billing.currentPeriodEnd)) : "-"}</strong></div>
                {dueCountdown ? (
                  <div className="grid min-w-0 gap-1 sm:grid-cols-[auto_minmax(0,1fr)] sm:items-start sm:gap-3"><span className="text-muted-foreground">{t("billing.timeRemaining")}</span><strong className="min-w-0 break-words sm:text-right">{dueCountdown}</strong></div>
                ) : null}
              </div>
              <p className="text-xs leading-5 text-muted-foreground">{t("billing.creationBlocked")}</p>
              {methods.length ? (
                <div className="grid min-w-0 grid-cols-[minmax(0,1fr)]"><div className="flex min-w-0 items-center gap-2"><WalletCards className="h-4 w-4 shrink-0" /><strong className="min-w-0 text-sm">{t("billing.providerPaymentDetails")}</strong></div>{methods.map((method, index) => <PaymentMethod key={`${method.kind}:${method.account || method.address || index}`} method={method} />)}</div>
              ) : (
                <div className="grid gap-2 rounded-md border border-dashed p-4 text-sm text-muted-foreground"><span>{t("billing.noPaymentDetails")}</span><a className="inline-flex items-center gap-2 font-medium text-primary hover:underline" href={`mailto:${billing.hostOwnerEmail}`}><Mail className="h-4 w-4" />{t("billing.emailOffline", { email: billing.hostOwnerEmail })}</a></div>
              )}
              <p className="text-xs leading-5 text-muted-foreground">{t("billing.declarationNotice")}</p>
            </Modal.Body>
            <Modal.Footer className="flex flex-wrap gap-2">
              <ReleaseRentalAction
                billing={billing}
                onChanged={onChanged}
                className="text-rose-700"
              />
              <div className="ml-auto flex flex-wrap gap-2">
                <Button variant="ghost" isDisabled={busy} onClick={() => setPaymentOpen(false)}>{t("common.close")}</Button>
                <Button variant="primary" isDisabled={busy || !billing.canDeclarePaid || !billing.openInvoiceId} onClick={() => setConfirmPayment(true)}>{t("billing.confirmPayment")}</Button>
              </div>
            </Modal.Footer>
          </Modal.Dialog>
        </Modal.Container>
      </Modal.Backdrop>

      <ConfirmAlertDialog
        open={confirmPayment}
        title={t("billing.confirmTitle")}
        description={t("billing.confirmDescription")}
        confirmLabel={t("billing.yesPaid")}
        cancelLabel={t("clientMarket.back")}
        busy={busy}
        tone="warning"
        onConfirm={() => void declarePaid()}
        onOpenChange={(next) => !busy && setConfirmPayment(next)}
      />
    </>
  );
}
