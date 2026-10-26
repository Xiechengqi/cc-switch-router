"use client";

import * as React from "react";
import { Button, Modal } from "@heroui/react";
import {
  Mail,
  MessagesSquare,
  Send,
  ShieldCheck,
} from "lucide-react";
import { ProviderContactsList } from "@/components/common/provider-contacts";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { ApiError, cancelMarketAccessRequest, createMarketAccessRequest } from "@/lib/api";
import type { MessageKey } from "@/lib/i18n";
import type {
  MarketAccessRequest,
  MarketEligibility,
  MarketEligibilityStatus,
  ProviderContact,
} from "@/lib/types";

const MARKET_ERROR_STATUS: Record<string, MarketEligibilityStatus> = {
  MARKET_ACCESS_REQUIRED: "access_required",

};

export function marketEligibilityFromError(reason: unknown): MarketEligibility | null {
  if (!(reason instanceof ApiError) || !reason.code) return null;
  const status = MARKET_ERROR_STATUS[reason.code];
  return status ? { allowed: false, status } : null;
}

function descriptionKey(status: MarketEligibilityStatus, share: boolean): MessageKey {
  if (status === "access_required") {
    return share ? "marketApproval.shareDescription" : "marketApproval.clientHostDescription";
  }

  return "marketApproval.unavailableDescription";
}

function titleKey(status: MarketEligibilityStatus): MessageKey {
  if (status === "access_required") return "marketApproval.accessTitle";

  return "marketApproval.unavailableTitle";
}

export function MarketAccessDialog({
  open,
  product,
  ownerEmail,
  buyerEmail,
  contacts,
  targetKind,
  targetId,
  currency,
  eligibility,
  onOpenChange,
  onOpenChat,
  onRequestChanged,
}: {
  open: boolean;
  product: "share" | "clientHost";
  ownerEmail: string;
  buyerEmail: string;
  contacts?: ProviderContact[] | null;
  targetKind: "share_seat" | "client_host";
  targetId: string;
  currency?: string;
  eligibility: MarketEligibility;
  onOpenChange: (open: boolean) => void;
  onOpenChat?: () => void | Promise<void>;
  onRequestChanged?: (request?: MarketAccessRequest) => void | Promise<void>;
}) {
  const { t } = useLocaleText();
  const share = product === "share";
  const [request, setRequest] = React.useState(eligibility.request);
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState("");

  React.useEffect(() => {
    if (!open) return;
    setRequest(eligibility.request);
    setBusy(false);
    setError("");
  }, [eligibility.request, open, targetId]);

  const contactOwner = () => {
    onOpenChange(false);
    if (share) {
      void onOpenChat?.();
      return;
    }
    const subject = encodeURIComponent(t("marketApproval.clientHostEmailSubject"));
    window.location.href = `mailto:${ownerEmail}?subject=${subject}`;
  };

  const apply = async () => {
    if (eligibility.status !== "access_required" || request || busy) return;
    setBusy(true);
    setError("");
    try {
      const created = await createMarketAccessRequest({ targetKind, targetId });
      setRequest({ id: created.id, status: created.status, revision: created.revision, requestedAt: created.requestedAt });
      await onRequestChanged?.(created);
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
    }
  };

  const cancelRequest = async () => {
    if (!request || busy) return;
    setBusy(true);
    setError("");
    try {
      await cancelMarketAccessRequest(request.id, request.revision);
      setRequest(undefined);
      await onRequestChanged?.();
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
    }
  };

  const canApply = eligibility.status === "access_required";
  const canContact = canApply;

  return (
    <Modal.Backdrop isOpen={open} onOpenChange={onOpenChange}>
      <Modal.Container placement="center">
        <Modal.Dialog className="light w-[min(540px,calc(100vw-2rem))] max-w-none !bg-white !text-slate-900">
          <Modal.Header>
            <Modal.Heading>{t(titleKey(eligibility.status))}</Modal.Heading>
          </Modal.Header>
          <Modal.Body className="grid gap-4">
            <div className="flex items-start gap-3">
              <span className="inline-flex h-9 w-9 shrink-0 items-center justify-center rounded-md bg-sky-50 text-sky-700">
                <ShieldCheck className="h-5 w-5" aria-hidden />
              </span>
              <div className="grid min-w-0 gap-1.5 break-words text-sm leading-6 text-slate-600">
                <p>
                  {t(descriptionKey(eligibility.status, share), {
                    owner: ownerEmail,
                    buyer: buyerEmail,
                    currency: currency || "-",
                  })}
                </p>
                {canApply ? (
                  <p>{t(share ? "marketApproval.shareContactHint" : "marketApproval.clientHostContactHint")}</p>
                ) : null}
                {request ? <p className="font-medium text-emerald-700">{t("marketApproval.requestedHint")}</p> : null}
                {error ? <p className="text-rose-700">{error}</p> : null}
              </div>
            </div>
            {!share && canContact ? (
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
            {canContact ? (
              <Button variant="outline" onClick={contactOwner}>
                {share ? <MessagesSquare className="h-4 w-4" /> : <Mail className="h-4 w-4" />}
                {t(share ? "marketApproval.openChat" : "marketApproval.emailOwner")}
              </Button>
            ) : null}
            {canApply && request ? (
              <Button variant="outline" isDisabled={busy} onClick={() => void cancelRequest()}>
                {t("marketApproval.cancelRequest")}
              </Button>
            ) : null}
            {canApply && !request ? (
              <Button variant="primary" isDisabled={!!request || busy} onClick={() => void apply()}>
                <Send className="h-4 w-4" />
                {request ? t("marketApproval.requested") : t("marketApproval.apply")}
              </Button>
            ) : null}
          </Modal.Footer>
        </Modal.Dialog>
      </Modal.Container>
    </Modal.Backdrop>
  );
}
