"use client";

import * as React from "react";
import { Button, Modal } from "@heroui/react";
import { Check, Copy, ExternalLink } from "lucide-react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import type { MessageKey } from "@/lib/i18n";

export function buildClientInstallCommand(origin = typeof window === "undefined" ? "https://[router_url]" : window.location.origin) {
  const base = origin.replace(/\/$/, "");
  return `curl -SsL ${base}/install-client.sh | sudo bash`;
}

export function InstallGuideDialog({
  open,
  onOpenChange,
  titleKey,
  descriptionKey,
  commandLabelKey,
  command,
  externalUrl,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  titleKey: MessageKey;
  descriptionKey: MessageKey;
  commandLabelKey: MessageKey;
  command: string;
  externalUrl?: string;
}) {
  const { t } = useLocaleText();
  const [copied, setCopied] = React.useState(false);

  const copy = React.useCallback(async () => {
    try {
      await navigator.clipboard.writeText(command);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 2000);
    } catch {
      setCopied(false);
    }
  }, [command]);

  return (
    <Modal isOpen={open} onOpenChange={onOpenChange}>
      <Modal.Backdrop>
        <Modal.Container>
          <Modal.Dialog className="w-[min(720px,calc(100vw-2rem))] max-w-none">
            <Modal.Header>
              <Modal.Heading>{t(titleKey)}</Modal.Heading>
            </Modal.Header>
            <Modal.Body className="grid gap-3">
              <p className="text-sm text-muted-foreground">{t(descriptionKey)}</p>
              <div className="rounded-lg border bg-slate-50 p-3">
                <div className="mb-2 font-mono text-[10px] uppercase tracking-[0.12em] text-muted-foreground">
                  {t(commandLabelKey)}
                </div>
                <pre className="overflow-x-auto whitespace-pre-wrap break-all font-mono text-[12px] leading-6 text-foreground">
                  {command}
                </pre>
              </div>
            </Modal.Body>
            <Modal.Footer>
              <Button variant="ghost" onClick={() => onOpenChange(false)}>
                {t("common.close")}
              </Button>
              {externalUrl ? (
                <Button
                  variant="outline"
                  onClick={() => {
                    window.open(externalUrl, "_blank", "noopener,noreferrer");
                  }}
                >
                  <ExternalLink className="h-4 w-4" />
                  {t("dashboard.installOpenLink")}
                </Button>
              ) : null}
              <Button variant="primary" onClick={() => void copy()}>
                {copied ? <Check className="h-4 w-4" /> : <Copy className="h-4 w-4" />}
                {copied ? t("dashboard.connectDialog.copyOk") : t("dashboard.connectDialog.copy")}
              </Button>
            </Modal.Footer>
          </Modal.Dialog>
        </Modal.Container>
      </Modal.Backdrop>
    </Modal>
  );
}
