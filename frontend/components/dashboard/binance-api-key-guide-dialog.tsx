"use client";

import { Button, Modal } from "@heroui/react";
import { ExternalLink } from "lucide-react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import type { MessageKey } from "@/lib/i18n";

const STEPS: Array<{
  title: MessageKey;
  body: MessageKey;
  bullets?: MessageKey[];
}> = [
  {
    title: "account.binanceAuto.guide.create.title",
    body: "account.binanceAuto.guide.create.body",
  },
  {
    title: "account.binanceAuto.guide.permissions.title",
    body: "account.binanceAuto.guide.permissions.check",
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

              <section className="border-t border-slate-200 pt-4">
                <h3 className="text-xs font-semibold text-slate-800">
                  {t("account.binanceAuto.guide.note.title")}
                </h3>
                <p className="mt-1 text-xs leading-5 text-muted-foreground">
                  {t("account.binanceAuto.guide.note.body")}
                </p>
              </section>
            </div>
          </Modal.Body>
          <Modal.Footer className="flex-wrap">
            <Button variant="ghost" onClick={() => onOpenChange(false)}>
              {t("common.close")}
            </Button>
            <Button
              variant="outline"
              onClick={() => window.open("https://www.binance.com/en/support/faq/how-to-create-api-keys-on-binance-360002502072", "_blank", "noopener,noreferrer")}
            >
              <ExternalLink className="h-4 w-4" aria-hidden />
              {t("account.binanceAuto.guide.openOfficialGuide")}
            </Button>
          </Modal.Footer>
        </Modal.Dialog>
      </Modal.Container>
    </Modal.Backdrop>
  );
}
