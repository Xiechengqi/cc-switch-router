"use client";

import * as React from "react";
import { Button, Modal } from "@heroui/react";
import { ContactRound } from "lucide-react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import type { MessageKey } from "@/lib/i18n";
import type { PaymentContact } from "@/lib/types";

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

/** Market contact glyph: opens a dialog of Provider contacts. Hidden when none are configured. */
export function ProviderContactButton({
  contacts,
  className,
}: {
  contacts?: PaymentContact[] | null;
  className?: string;
}) {
  const { t } = useLocaleText();
  const [open, setOpen] = React.useState(false);
  const items = (contacts || []).filter((contact) => contact.handle?.trim());
  if (!items.length) return null;

  return (
    <>
      <Button
        isIconOnly
        size="sm"
        variant="ghost"
        className={`h-7 w-7 min-w-7 shrink-0 border-0 shadow-none text-foreground ${className || ""}`}
        aria-label={t("account.contact.view")}
        data-no-row-drawer
        onClick={(event) => {
          event.stopPropagation();
          setOpen(true);
        }}
      >
        <ContactRound className="h-3.5 w-3.5" />
      </Button>
      <Modal.Backdrop isOpen={open} onOpenChange={setOpen}>
        <Modal.Container placement="center">
          <Modal.Dialog className="light w-[min(420px,calc(100vw-2rem))] max-w-none !bg-white !text-slate-900">
            <Modal.Header>
              <Modal.Heading>{t("account.contact.title")}</Modal.Heading>
            </Modal.Header>
            <Modal.Body className="grid gap-3">
              <ProviderContactsList contacts={items} />
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
