"use client";

import * as React from "react";
import { Button, Chip, Modal, toast } from "@heroui/react";
import { Clock3, Loader2, Mail, Trash2, WalletCards } from "lucide-react";
import { ConfirmAlertDialog } from "@/components/common/confirm-alert-dialog";
import { AuthenticatedImage } from "@/components/common/authenticated-image";
import { PaymentMethodIcons } from "@/components/common/payment-method-icons";
import { useLocaleText } from "@/components/i18n/locale-provider";
import {
  cleanupClientMarketClientWithReason,
  declareClientMarketPayment,
} from "@/lib/api";
import type { ClientMarketBilling, ClientMarketPaymentMethod } from "@/lib/types";

function countdown(value: string | undefined, locale: string) {
  if (!value) return "";
  const milliseconds = Date.parse(value) - Date.now();
  if (!Number.isFinite(milliseconds) || milliseconds <= 0) return locale.startsWith("zh") ? "已逾期" : "overdue";
  const minutes = Math.ceil(milliseconds / 60_000);
  const days = Math.floor(minutes / 1_440);
  const hours = Math.floor((minutes % 1_440) / 60);
  const remainingMinutes = minutes % 60;
  if (locale.startsWith("zh")) {
    if (days) return `${days}天 ${hours}小时`;
    if (hours) return `${hours}小时 ${remainingMinutes}分钟`;
    return `${remainingMinutes}分钟`;
  }
  if (days) return `${days}d ${hours}h`;
  if (hours) return `${hours}h ${remainingMinutes}m`;
  return `${remainingMinutes}m`;
}

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

export function ClientMarketBillingBanner({
  billing,
  onChanged,
}: {
  billing?: ClientMarketBilling;
  onChanged: () => Promise<void> | void;
}) {
  const { locale, t } = useLocaleText();
  const [paymentOpen, setPaymentOpen] = React.useState(false);
  const [confirmPayment, setConfirmPayment] = React.useState(false);
  const [confirmRelease, setConfirmRelease] = React.useState(false);
  const [busy, setBusy] = React.useState(false);
  const [, tick] = React.useState(0);

  React.useEffect(() => {
    if (!billing || (billing.status !== "payment_due" && billing.status !== "active")) return;
    const timer = window.setInterval(() => tick((value) => value + 1), 30_000);
    return () => window.clearInterval(timer);
  }, [billing]);

  if (!billing || !billing.isClientOwner || billing.status === "released") return null;

  const declarePaid = async () => {
    if (!billing.openInvoiceId) return;
    setBusy(true);
    try {
      await declareClientMarketPayment(
        billing.installationId,
        billing.openInvoiceId,
        billing.offerRevision,
        billing.paymentProfileUpdatedAt,
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

  const release = async () => {
    setBusy(true);
    try {
      await cleanupClientMarketClientWithReason(billing.installationId, {
        reason: "client_release",
        blockClientForProvider: false,
      });
      toast.info(t("billing.releaseStartedToast"));
      setConfirmRelease(false);
      await onChanged();
    } catch (reason) {
      toast.danger(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
    }
  };

  const methods = billing.paymentMethods || [];
  const dueCountdown = countdown(billing.paymentDeadline, locale);
  const renewalCountdown = countdown(billing.currentPeriodEnd, locale);

  return (
    <>
      {billing.status === "payment_due" ? (
        <div className="flex min-w-0 flex-wrap items-center gap-2 border-b border-amber-200 bg-amber-50 px-3.5 py-2">
          <Button size="sm" variant="outline" className="border-amber-300 bg-white text-amber-800" onClick={() => setPaymentOpen(true)}>
            <Clock3 className="h-4 w-4" />
            {t("billing.paymentDue")} · {dueCountdown || t("billing.threeDays")}
          </Button>
          <Button size="sm" variant="ghost" className="text-rose-700" onClick={() => setConfirmRelease(true)}>
            <Trash2 className="h-4 w-4" />
            {t("billing.releaseNow")}
          </Button>
          <span className="min-w-0 break-words text-xs text-amber-800">{t("billing.creationBlocked")}</span>
        </div>
      ) : billing.status === "releasing" ? (
        <div className="flex items-center gap-2 border-b bg-slate-50 px-3.5 py-2 text-xs text-muted-foreground"><Loader2 className="h-4 w-4 animate-spin" />{t("billing.releasing")}</div>
      ) : billing.status === "release_failed" ? (
        <div className="flex flex-wrap items-center gap-2 border-b border-rose-200 bg-rose-50 px-3.5 py-2 text-xs text-rose-700"><span>{t("billing.releaseFailed")}</span><Button size="sm" variant="outline" onClick={() => setConfirmRelease(true)}>{t("billing.retryRelease")}</Button></div>
      ) : billing.status === "active" && billing.priceCents && billing.currentPeriodEnd ? (
        <div className="flex items-center gap-2 border-b bg-slate-50 px-3.5 py-1.5 text-xs text-muted-foreground"><Clock3 className="h-3.5 w-3.5" />{t("billing.nextWindow", { countdown: renewalCountdown, date: new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short" }).format(new Date(billing.currentPeriodEnd)) })}</div>
      ) : null}

      <Modal.Backdrop isOpen={paymentOpen} onOpenChange={(next) => !busy && setPaymentOpen(next)}>
        <Modal.Container placement="center">
          <Modal.Dialog className="light min-w-0 w-[min(620px,calc(100vw-2rem))] max-w-none overflow-hidden !bg-white !text-slate-900">
            <Modal.Header><Modal.Heading>{t("billing.payProvider")}</Modal.Heading></Modal.Header>
            <Modal.Body className="grid min-w-0 max-h-[70vh] grid-cols-[minmax(0,1fr)] gap-4 overflow-y-auto">
              <div className="grid min-w-0 grid-cols-[minmax(0,1fr)] gap-2 rounded-md border border-amber-200 bg-amber-50 p-3 text-sm">
                <div className="grid min-w-0 gap-1 sm:grid-cols-[auto_minmax(0,1fr)] sm:items-start sm:gap-3"><span className="text-muted-foreground">{t("billing.provider")}</span><strong className="min-w-0 break-all sm:text-right">{billing.hostOwnerEmail}</strong></div>
                <div className="grid min-w-0 gap-1 sm:grid-cols-[auto_minmax(0,1fr)] sm:items-start sm:gap-3"><span className="text-muted-foreground">{t("billing.currentOffer")}</span><strong className="min-w-0 break-words sm:text-right">{offerLabel(billing, locale)}</strong></div>
                <div className="grid min-w-0 gap-1 sm:grid-cols-[auto_minmax(0,1fr)] sm:items-start sm:gap-3"><span className="text-muted-foreground">{t("billing.declareBefore")}</span><strong className="min-w-0 break-words sm:text-right">{billing.paymentDeadline ? new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short" }).format(new Date(billing.paymentDeadline)) : "-"}</strong></div>
              </div>
              {methods.length ? (
                <div className="grid min-w-0 grid-cols-[minmax(0,1fr)]"><div className="flex min-w-0 items-center gap-2"><WalletCards className="h-4 w-4 shrink-0" /><strong className="min-w-0 text-sm">{t("billing.providerPaymentDetails")}</strong></div>{methods.map((method, index) => <PaymentMethod key={`${method.kind}:${method.account || method.address || index}`} method={method} />)}</div>
              ) : (
                <div className="grid gap-2 rounded-md border border-dashed p-4 text-sm text-muted-foreground"><span>{t("billing.noPaymentDetails")}</span><a className="inline-flex items-center gap-2 font-medium text-primary hover:underline" href={`mailto:${billing.hostOwnerEmail}`}><Mail className="h-4 w-4" />{t("billing.emailOffline", { email: billing.hostOwnerEmail })}</a></div>
              )}
              <p className="text-xs leading-5 text-muted-foreground">{t("billing.declarationNotice")}</p>
            </Modal.Body>
            <Modal.Footer><Button variant="ghost" isDisabled={busy} onClick={() => setPaymentOpen(false)}>{t("common.close")}</Button><Button variant="primary" isDisabled={busy || !billing.canDeclarePaid || !billing.openInvoiceId} onClick={() => setConfirmPayment(true)}>{t("billing.confirmPayment")}</Button></Modal.Footer>
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
      <ConfirmAlertDialog
        open={confirmRelease}
        title={t("billing.releaseTitle")}
        description={t("billing.releaseDescription")}
        confirmLabel={t("billing.releaseClient")}
        cancelLabel={t("common.cancel")}
        busy={busy}
        tone="danger"
        onConfirm={() => void release()}
        onOpenChange={(next) => !busy && setConfirmRelease(next)}
      />
    </>
  );
}
