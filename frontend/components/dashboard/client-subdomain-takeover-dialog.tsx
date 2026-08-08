"use client";

import { Button, Input, Modal, toast } from "@heroui/react";
import { ArrowRightLeft, Loader2, TriangleAlert } from "lucide-react";
import * as React from "react";

import { CompactSelect } from "@/components/common/compact-select";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { takeOverClientSubdomain } from "@/lib/api";
import type { ClientSubdomainTakeoverResponse, DashboardClient } from "@/lib/types";

function clientLabel(client: DashboardClient) {
  return client.clientTunnel?.subdomain || client.installation.id;
}

export function ClientSubdomainTakeoverDialog({
  target,
  sources,
  open,
  onOpenChange,
  onCompleted,
}: {
  target: DashboardClient | null;
  sources: DashboardClient[];
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onCompleted: (response: ClientSubdomainTakeoverResponse) => Promise<void> | void;
}) {
  const { t } = useLocaleText();
  const [sourceId, setSourceId] = React.useState("");
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState("");
  const [confirmation, setConfirmation] = React.useState("");

  React.useEffect(() => {
    if (!open) return;
    setSourceId(sources[0]?.installation.id || "");
    setConfirmation("");
    setError("");
  }, [open, sources]);

  const source = sources.find((client) => client.installation.id === sourceId) || null;
  const submit = async () => {
    if (!target || !source) return;
    setBusy(true);
    setError("");
    try {
      const response = await takeOverClientSubdomain({
        targetInstallationId: target.installation.id,
        sourceInstallationId: source.installation.id,
      });
      toast.success(t("dashboard.subdomainTakeover.success", { subdomain: response.adoptedSubdomain }));
      if (response.warning) toast.warning(response.warning);
      await onCompleted(response);
      onOpenChange(false);
    } catch (reason) {
      const message = reason instanceof Error ? reason.message : String(reason);
      setError(message);
      toast.danger(message);
    } finally {
      setBusy(false);
    }
  };

  const targetSubdomain = target?.clientTunnel?.subdomain || "-";
  const sourceSubdomain = source?.clientTunnel?.subdomain || "-";
  const confirmationMatches = sourceSubdomain !== "-" && confirmation.trim() === sourceSubdomain;

  return (
    <Modal.Backdrop isOpen={open} onOpenChange={(next) => !busy && onOpenChange(next)}>
      <Modal.Container placement="center">
        <Modal.Dialog className="light w-[min(600px,calc(100vw-2rem))] max-w-none !bg-white !text-slate-900">
          <Modal.CloseTrigger />
          <Modal.Header>
            <Modal.Heading>{t("dashboard.subdomainTakeover.title")}</Modal.Heading>
          </Modal.Header>
          <Modal.Body className="grid gap-4">
            <div className="grid gap-1 text-sm text-slate-600">
              <span>{t("dashboard.subdomainTakeover.target")}</span>
              <strong className="break-all font-mono text-slate-900">{target ? clientLabel(target) : "-"}</strong>
            </div>

            <label className="grid gap-1.5 text-sm text-slate-600">
              {t("dashboard.subdomainTakeover.source")}
              <CompactSelect
                value={sourceId}
                onChange={setSourceId}
                options={sources.map((client) => ({
                  value: client.installation.id,
                  label: clientLabel(client),
                }))}
                ariaLabel={t("dashboard.subdomainTakeover.source")}
                className="w-full"
              />
            </label>

            <div className="grid grid-cols-[minmax(0,1fr)_auto_minmax(0,1fr)] items-center gap-3 rounded-md border border-slate-200 bg-slate-50 px-3 py-3">
              <div className="min-w-0">
                <div className="text-xs text-slate-500">{t("dashboard.subdomainTakeover.retired")}</div>
                <div className="truncate font-mono text-sm font-semibold" title={targetSubdomain}>{targetSubdomain}</div>
              </div>
              <ArrowRightLeft className="h-4 w-4 shrink-0 text-slate-400" aria-hidden />
              <div className="min-w-0 text-right">
                <div className="text-xs text-slate-500">{t("dashboard.subdomainTakeover.adopted")}</div>
                <div className="truncate font-mono text-sm font-semibold" title={sourceSubdomain}>{sourceSubdomain}</div>
              </div>
            </div>

            <div className="flex items-start gap-3 rounded-md border border-rose-200 bg-rose-50 px-3 py-3 text-sm leading-6 text-rose-900">
              <TriangleAlert className="mt-0.5 h-4 w-4 shrink-0" aria-hidden />
              <div className="grid gap-1">
                <p>{t("dashboard.subdomainTakeover.warningSource")}</p>
                <p>{t("dashboard.subdomainTakeover.warningData")}</p>
                <p>{t("dashboard.subdomainTakeover.warningShares")}</p>
              </div>
            </div>
            <label className="grid gap-1.5 text-sm text-slate-700">
              <span>{t("dashboard.subdomainTakeover.confirmPrompt", { subdomain: sourceSubdomain })}</span>
              <Input
                value={confirmation}
                onChange={(event) => setConfirmation(event.target.value)}
                autoComplete="off"
                autoCapitalize="none"
                spellCheck={false}
                placeholder={sourceSubdomain}
                aria-label={t("dashboard.subdomainTakeover.confirmInput")}
              />
            </label>
            {error ? <p className="break-words text-sm text-rose-700" role="alert">{error}</p> : null}
          </Modal.Body>
          <Modal.Footer>
            <Button variant="ghost" isDisabled={busy} onClick={() => onOpenChange(false)}>
              {t("common.cancel")}
            </Button>
            <Button variant="danger" isDisabled={busy || !source || !confirmationMatches} onClick={() => void submit()}>
              {busy ? <Loader2 className="h-4 w-4 animate-spin" /> : <ArrowRightLeft className="h-4 w-4" />}
              {busy ? t("dashboard.subdomainTakeover.running") : t("dashboard.subdomainTakeover.confirm")}
            </Button>
          </Modal.Footer>
        </Modal.Dialog>
      </Modal.Container>
    </Modal.Backdrop>
  );
}
