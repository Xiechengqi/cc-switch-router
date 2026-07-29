"use client";

import * as React from "react";
import { Button, Chip, Modal, Tooltip } from "@heroui/react";
import { ContactRound, WalletCards } from "lucide-react";
import { AuthenticatedImage } from "@/components/common/authenticated-image";
import { PaymentMethodIcons } from "@/components/common/payment-method-icons";
import { useLocaleText } from "@/components/i18n/locale-provider";
import type { MessageKey } from "@/lib/i18n";
import type { ClientMarketPaymentMethod, PaymentContact } from "@/lib/types";

export function contactChannelLabelKey(channel: string): MessageKey {
  switch (channel) {
    case "wechat":
      return "account.contact.channel.wechat";
    case "telegram":
      return "account.contact.channel.telegram";
    case "custom":
      return "account.contact.channel.custom";
    default:
      return "account.contact.channel.custom";
  }
}

/** Read-only list used inside payment dialogs. */
export function ProviderContactsList({ contacts }: { contacts?: PaymentContact[] | null }) {
  const { t } = useLocaleText();
  const items = (contacts || []).filter((contact) => contact.handle?.trim());
  if (!items.length) return null;
  return (
    <div className="grid min-w-0 gap-2 rounded-md border border-border bg-slate-50 p-3">
      <strong className="text-sm">{t("account.contact.title")}</strong>
      <ul className="grid gap-1.5 text-sm">
        {items.map((contact, index) => (
          <li
            key={`${contact.channel}:${contact.handle}:${index}`}
            className="flex min-w-0 flex-wrap items-baseline gap-x-2 gap-y-0.5"
          >
            <span className="shrink-0 text-muted-foreground">{t(contactChannelLabelKey(contact.channel))}</span>
            <code className="min-w-0 break-all font-medium text-foreground">{contact.handle}</code>
          </li>
        ))}
      </ul>
    </div>
  );
}

function paymentMethodLabel(method: ClientMarketPaymentMethod, t: ReturnType<typeof useLocaleText>["t"]) {
  switch (method.kind) {
    case "alipay":
      return t("billing.payment.alipay");
    case "wechat":
      return t("billing.payment.wechat");
    case "binance":
      return t("billing.payment.binance");
    case "crypto":
      return t("billing.payment.crypto");
    case "custom":
      return t("billing.payment.custom");
    default:
      return method.kind;
  }
}

export function ProviderPaymentMethodsList({
  paymentMethods,
}: {
  paymentMethods?: ClientMarketPaymentMethod[] | null;
}) {
  const { t } = useLocaleText();
  const methods = (paymentMethods || []).filter((method) => method.kind?.trim());
  if (!methods.length) return null;

  return (
    <div className="grid min-w-0 gap-2 rounded-md border border-border bg-slate-50 p-3">
      <div className="flex min-w-0 items-center gap-2">
        <WalletCards className="h-4 w-4 shrink-0 text-muted-foreground" aria-hidden />
        <strong className="text-sm">{t("billing.providerPaymentDetails")}</strong>
      </div>
      <div className="grid min-w-0 gap-3">
        {methods.map((method, index) => {
          const label = paymentMethodLabel(method, t);
          return (
            <section
              key={`${method.kind}:${method.account || method.address || index}`}
              className="grid min-w-0 gap-2 border-b border-slate-200 pb-3 last:border-0 last:pb-0"
            >
              <div className="flex min-w-0 flex-wrap items-center gap-2">
                <PaymentMethodIcons kinds={[method.kind]} />
                <strong className="text-sm">{label}</strong>
                {method.token ? <Chip size="sm" variant="soft">{method.token}</Chip> : null}
                {method.chain ? <Chip size="sm" variant="soft">{method.chain.toUpperCase()}</Chip> : null}
              </div>
              {method.account ? (
                <div className="min-w-0 break-words text-sm">
                  <span className="text-muted-foreground">{t("billing.account")}: </span>
                  <span className="font-medium text-foreground">{method.account}</span>
                </div>
              ) : null}
              {method.address ? (
                <code className="break-all rounded-md bg-white px-3 py-2 text-xs text-foreground">
                  {method.address}
                </code>
              ) : null}
              {method.instructions ? (
                <p className="whitespace-pre-wrap break-words text-sm leading-6 text-slate-700">
                  {method.instructions}
                </p>
              ) : null}
              {method.assetUrl ? (
                <AuthenticatedImage
                  src={method.assetUrl}
                  alt={t("billing.qrAlt", { method: label })}
                  className="h-40 w-40 rounded-md border bg-white object-contain p-1"
                />
              ) : null}
            </section>
          );
        })}
      </div>
    </div>
  );
}

/** Market contact glyph: opens the Provider contact and payment details. */
export function ProviderContactButton({
  contacts,
  paymentMethods,
  className,
}: {
  contacts?: PaymentContact[] | null;
  paymentMethods?: ClientMarketPaymentMethod[] | null;
  className?: string;
}) {
  const { t } = useLocaleText();
  const [open, setOpen] = React.useState(false);
  const items = (contacts || []).filter((contact) => contact.handle?.trim());
  const methods = (paymentMethods || []).filter((method) => method.kind?.trim());
  if (!items.length && !methods.length) return null;

  return (
    <>
      <Tooltip>
        <Tooltip.Trigger>
          <Button
            isIconOnly
            size="sm"
            variant="ghost"
            className={`h-7 w-7 min-w-7 shrink-0 border-0 shadow-none text-foreground ${className || ""}`}
            aria-label={t("account.contactPayment.view")}
            data-no-row-drawer
            onClick={(event) => {
              event.stopPropagation();
              setOpen(true);
            }}
          >
            <ContactRound className="h-3.5 w-3.5" />
          </Button>
        </Tooltip.Trigger>
        <Tooltip.Content>{t("account.contactPayment.view")}</Tooltip.Content>
      </Tooltip>
      <Modal.Backdrop isOpen={open} onOpenChange={setOpen}>
        <Modal.Container placement="center">
          <Modal.Dialog className="light w-[min(420px,calc(100vw-2rem))] max-w-none !bg-white !text-slate-900">
            <Modal.Header>
              <Modal.Heading>{t("account.contactPayment.title")}</Modal.Heading>
            </Modal.Header>
            <Modal.Body className="grid max-h-[70vh] gap-3 overflow-y-auto">
              <ProviderContactsList contacts={items} />
              <ProviderPaymentMethodsList paymentMethods={methods} />
            </Modal.Body>
            <Modal.Footer>
              <Button variant="ghost" onClick={() => setOpen(false)}>
                {t("common.close")}
              </Button>
            </Modal.Footer>
          </Modal.Dialog>
        </Modal.Container>
      </Modal.Backdrop>
    </>
  );
}
