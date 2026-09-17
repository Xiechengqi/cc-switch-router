"use client";

import { Button, Modal } from "@heroui/react";
import { ExternalLink, ShieldCheck, TriangleAlert } from "lucide-react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import type { MessageKey } from "@/lib/i18n";

const STEPS: Array<{
  title: MessageKey;
  body: MessageKey;
  bullets?: MessageKey[];
}> = [
  {
    title: "account.binanceAuto.guide.uid.title",
    body: "account.binanceAuto.guide.uid.body",
  },
  {
    title: "account.binanceAuto.guide.proof.title",
    body: "account.binanceAuto.guide.proof.body",
  },
  {
    title: "account.binanceAuto.guide.create.title",
    body: "account.binanceAuto.guide.create.body",
  },
  {
    title: "account.binanceAuto.guide.permissions.title",
    body: "account.binanceAuto.guide.permissions.body",
    bullets: [
      "account.binanceAuto.guide.permissions.read",
      "account.binanceAuto.guide.permissions.block",
      "account.binanceAuto.guide.permissions.ip",
    ],
  },
  {
    title: "account.binanceAuto.guide.copy.title",
    body: "account.binanceAuto.guide.copy.body",
  },
  {
    title: "account.binanceAuto.guide.verify.title",
    body: "account.binanceAuto.guide.verify.body",
  },
];

const ERRORS: Array<{ code: string; message: MessageKey }> = [
  { code: "READ_PERMISSION_REQUIRED", message: "account.binanceAuto.guide.error.read" },
  { code: "DANGEROUS_PERMISSION_ENABLED", message: "account.binanceAuto.guide.error.dangerous" },
  { code: "ACCOUNT_UID_UNCONFIRMED", message: "account.binanceAuto.guide.error.unconfirmed" },
  { code: "ACCOUNT_UID_AMBIGUOUS", message: "account.binanceAuto.guide.error.ambiguous" },
  { code: "ACCOUNT_UID_MISMATCH", message: "account.binanceAuto.guide.error.mismatch" },
  { code: "BINANCE_CREDENTIALS_REJECTED", message: "account.binanceAuto.guide.error.credentials" },
];

export function BinanceApiKeyGuideDialog({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const { t } = useLocaleText();

  return (
    <Modal.Backdrop isOpen={open} onOpenChange={onOpenChange}>
      <Modal.Container placement="center">
        <Modal.Dialog className="light flex max-h-[min(90vh,calc(100vh-1.5rem))] w-[min(760px,calc(100vw-1.5rem))] max-w-none flex-col overflow-hidden !bg-white !text-slate-900">
          <Modal.CloseTrigger className="!bg-slate-100 !text-slate-700 hover:!bg-slate-200 hover:!text-slate-950" />
          <Modal.Header>
            <Modal.Heading className="!text-slate-900">
              {t("account.binanceAuto.guide.title")}
            </Modal.Heading>
          </Modal.Header>
          <Modal.Body className="min-h-0 overflow-y-auto !text-slate-900">
            <div className="grid gap-5">
              <div className="flex gap-3 rounded-lg border border-amber-200 bg-amber-50 p-3 text-amber-950">
                <ShieldCheck className="mt-0.5 h-5 w-5 shrink-0" aria-hidden />
                <div>
                  <strong className="text-sm">{t("account.binanceAuto.guide.safetyTitle")}</strong>
                  <p className="mt-1 text-xs leading-5">
                    {t("account.binanceAuto.guide.safetyBody")}
                  </p>
                </div>
              </div>

              <ol className="grid gap-4">
                {STEPS.map((step, index) => (
                  <li key={step.title} className="flex gap-3">
                    <span className="flex h-6 w-6 shrink-0 items-center justify-center rounded-full bg-slate-900 text-xs font-semibold text-white">
                      {index + 1}
                    </span>
                    <div className="min-w-0">
                      <h3 className="text-sm font-semibold">{t(step.title)}</h3>
                      <p className="mt-1 text-xs leading-5 text-muted-foreground">{t(step.body)}</p>
                      {step.bullets ? (
                        <ul className="mt-2 list-disc space-y-1 pl-5 text-xs leading-5 text-slate-700">
                          {step.bullets.map((bullet) => <li key={bullet}>{t(bullet)}</li>)}
                        </ul>
                      ) : null}
                    </div>
                  </li>
                ))}
              </ol>

              <section className="rounded-lg border border-slate-200 bg-slate-50 p-3">
                <div className="flex items-center gap-2">
                  <TriangleAlert className="h-4 w-4 text-amber-700" aria-hidden />
                  <h3 className="text-sm font-semibold">{t("account.binanceAuto.guide.errors.title")}</h3>
                </div>
                <dl className="mt-3 grid gap-2">
                  {ERRORS.map((error) => (
                    <div key={error.code} className="grid gap-0.5 sm:grid-cols-[15rem_1fr] sm:gap-3">
                      <dt className="break-all font-mono text-[11px] font-semibold text-slate-800">{error.code}</dt>
                      <dd className="text-xs leading-5 text-muted-foreground">{t(error.message)}</dd>
                    </div>
                  ))}
                </dl>
              </section>

              <p className="text-xs leading-5 text-muted-foreground">
                {t("account.binanceAuto.guide.uiNotice")}
              </p>
            </div>
          </Modal.Body>
          <Modal.Footer className="flex-wrap">
            <Button variant="ghost" onClick={() => onOpenChange(false)}>
              {t("common.close")}
            </Button>
            <Button
              variant="outline"
              onClick={() => window.open("https://www.binance.com/en/my/settings/api-management", "_blank", "noopener,noreferrer")}
            >
              <ExternalLink className="h-4 w-4" aria-hidden />
              {t("account.binanceAuto.guide.openApiManagement")}
            </Button>
            <Button
              variant="outline"
              onClick={() => window.open("https://developers.binance.com/docs/pay/rest-api", "_blank", "noopener,noreferrer")}
            >
              <ExternalLink className="h-4 w-4" aria-hidden />
              {t("account.binanceAuto.guide.openApiDocs")}
            </Button>
          </Modal.Footer>
        </Modal.Dialog>
      </Modal.Container>
    </Modal.Backdrop>
  );
}
