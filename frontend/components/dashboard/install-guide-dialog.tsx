"use client";

import * as React from "react";
import { Button, Modal } from "@heroui/react";
import { Check, Copy, ExternalLink } from "lucide-react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import type { MessageKey } from "@/lib/i18n";

export function buildClientInstallCommand(options?: {
  origin?: string;
  ownerEmail?: string;
  passwordPlaceholder?: string;
}) {
  const base = (options?.origin ?? (typeof window === "undefined" ? "https://[router_url]" : window.location.origin)).replace(
    /\/$/,
    "",
  );
  const ownerEmail = options?.ownerEmail?.trim() || "owner@example.com";
  const password = options?.passwordPlaceholder?.trim() || "web登陆密码";
  return `curl -SsL ${base}/install-client.sh | bash -s ${base} ${ownerEmail} ${password}`;
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
    <Modal.Backdrop isOpen={open} onOpenChange={onOpenChange}>
        <Modal.Container>
          <Modal.Dialog className="light w-[min(720px,calc(100vw-2rem))] max-w-none !bg-white !text-slate-900">
            <Modal.Header>
              <Modal.Heading className="!text-slate-900">{t(titleKey)}</Modal.Heading>
            </Modal.Header>
            <Modal.Body className="grid gap-3 text-slate-900">
              <p className="text-sm text-muted-foreground">{t(descriptionKey)}</p>
              <div className="rounded-lg border bg-slate-50 p-3">
                <div className="mb-2 font-mono text-[10px] uppercase tracking-[0.12em] text-muted-foreground">
                  {t(commandLabelKey)}
                </div>
                <pre className="overflow-x-auto whitespace-pre-wrap break-all font-mono text-[12px] leading-6 text-slate-900">
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
  );
}
