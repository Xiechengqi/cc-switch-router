"use client";

import * as React from "react";
import { Button, Modal } from "@heroui/react";
import { Mail, MessagesSquare, ShieldCheck } from "lucide-react";
import { ProviderContactsList } from "@/components/common/provider-contacts";
import { useLocaleText } from "@/components/i18n/locale-provider";
import type { PaymentContact } from "@/lib/types";

export const SELLER_APPROVAL_REQUIRED_MESSAGE =
  "seller approval is required before renting this market service";

export function isSellerApprovalRequiredError(reason: unknown) {
  return reason instanceof Error && reason.message.includes(SELLER_APPROVAL_REQUIRED_MESSAGE);
}

export function SellerApprovalDialog({
  open,
  product,
  ownerEmail,
  buyerEmail,
  contacts,
  onOpenChange,
  onOpenChat,
}: {
  open: boolean;
  product: "share" | "clientHost";
  ownerEmail: string;
  buyerEmail: string;
  contacts?: PaymentContact[] | null;
  onOpenChange: (open: boolean) => void;
  onOpenChat?: () => void | Promise<void>;
}) {
  const { t } = useLocaleText();
  const share = product === "share";

  const contactOwner = () => {
    onOpenChange(false);
    if (share) {
      void onOpenChat?.();
      return;
    }
    const subject = encodeURIComponent(t("marketApproval.clientHostEmailSubject"));
    window.location.href = `mailto:${ownerEmail}?subject=${subject}`;
  };

  return (
    <Modal.Backdrop isOpen={open} onOpenChange={onOpenChange}>
      <Modal.Container placement="center">
        <Modal.Dialog className="light w-[min(520px,calc(100vw-2rem))] max-w-none !bg-white !text-slate-900">
          <Modal.Header>
            <Modal.Heading>{t("marketApproval.title")}</Modal.Heading>
          </Modal.Header>
          <Modal.Body className="grid gap-4">
            <div className="flex items-start gap-3">
              <span className="inline-flex h-9 w-9 shrink-0 items-center justify-center rounded-md bg-sky-50 text-sky-700">
                <ShieldCheck className="h-5 w-5" aria-hidden />
              </span>
              <div className="grid min-w-0 gap-1.5 break-words text-sm leading-6 text-slate-600">
                <p>
                  {t(share ? "marketApproval.shareDescription" : "marketApproval.clientHostDescription", {
                    owner: ownerEmail,
                    buyer: buyerEmail,
                  })}
                </p>
                <p>{t(share ? "marketApproval.shareContactHint" : "marketApproval.clientHostContactHint")}</p>
              </div>
            </div>
            {!share ? (
              <div className="grid gap-3">
                <a
                  href={`mailto:${ownerEmail}`}
                  className="inline-flex min-w-0 items-center gap-2 text-sm font-medium text-slate-900 underline-offset-2 hover:underline"
                >
                  <Mail className="h-4 w-4 shrink-0 text-slate-500" aria-hidden />
                  <span className="min-w-0 break-all">{ownerEmail}</span>
                </a>
                <ProviderContactsList contacts={contacts} />
              </div>
            ) : null}
          </Modal.Body>
          <Modal.Footer>
            <Button variant="ghost" onClick={() => onOpenChange(false)}>
              {t("common.close")}
            </Button>
            <Button variant="primary" onClick={contactOwner}>
              {share ? <MessagesSquare className="h-4 w-4" /> : <Mail className="h-4 w-4" />}
              {t(share ? "marketApproval.openChat" : "marketApproval.emailOwner")}
            </Button>
          </Modal.Footer>
        </Modal.Dialog>
      </Modal.Container>
    </Modal.Backdrop>
  );
}
