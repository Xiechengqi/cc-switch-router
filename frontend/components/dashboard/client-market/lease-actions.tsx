"use client";

import * as React from "react";
import { Button, Modal, TextArea } from "@heroui/react";
import { CompactSelect } from "@/components/common/compact-select";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { ClientUpgradeControl } from "@/components/dashboard/client-upgrade-button";
import { ApiError } from "@/lib/api";
import { authFetch } from "@/lib/auth";
import { onlineMarketError, onlineRequest } from "@/lib/online-client-market";
import { formatWalletUnits } from "@/lib/market-wallet-money";
import {
  previewClientTermination,
  terminationExplanationValid,
  terminationPreviewExpired,
  type ClientTerminationPreviewRead,
  type ClientTerminationRequest,
} from "@/lib/client-termination";

export function LeaseActions({
  id,
  active,
  upgradeTarget,
  canEnd,
  address,
  showConsole = true,
  onChanged,
}: {
  id: string;
  active: boolean;
  upgradeTarget?: string;
  canEnd: boolean;
  address?: string;
  /** Off where the caller already shows a prominent console button. */
  showConsole?: boolean;
  onChanged: () => void;
}) {
  const { t, locale } = useLocaleText();
  const [ending, setEnding] = React.useState(false);
  const [preview, setPreview] = React.useState<ClientTerminationPreviewRead | null>(null);
  const [reason, setReason] = React.useState<ClientTerminationRequest["reason"]>("voluntary");
  const [claim, setClaim] = React.useState("");
  const [busy, setBusy] = React.useState(false);
  const [message, setMessage] = React.useState("");
  const [error, setError] = React.useState("");
  const openHref = "/client-market/leases/" + encodeURIComponent(id) + "/open";
  const reset = () => {
    setError("");
    setMessage("");
    setPreview(null);
    setClaim("");
    setReason("voluntary");
  };
  async function run(fn: () => Promise<void>) {
    if (busy) return;
    setBusy(true);
    setError("");
    setMessage("");
    try {
      await fn();
    } catch (e) {
      setError(onlineMarketError(e, t));
    } finally {
      setBusy(false);
    }
  }
  async function backup() {
    const response = await authFetch(
      "/v1/client-market/online/leases/" + encodeURIComponent(id) + "/backup",
      { method: "POST", cache: "no-store" },
    );
    if (!response.ok || response.headers.get("content-type") !== "application/zip")
      throw new Error(t("lease.backupFailed"));
    const url = URL.createObjectURL(await response.blob());
    const a = document.createElement("a");
    a.href = url;
    a.download = "workspace-" + id + ".zip";
    a.click();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
    setMessage(t("lease.downloaded"));
  }
  const status = (
    <>
      {busy && <span role="status">{t("common.loading")}</span>}
      {message && <span role="status">{message}</span>}
      {error && (
        <span role="alert" className="text-danger">
          {error}
        </span>
      )}
    </>
  );
  const exportButton = (
    <Button size="sm" variant="outline" isDisabled={busy} onPress={() => void run(backup)}>
      {t("lease.backup")}
    </Button>
  );
  return (
    <div className="flex flex-wrap items-center gap-2 text-sm">
      {active && showConsole && (
        <>
          <a
            className="inline-flex h-9 items-center rounded font-medium text-accent underline"
            href={openHref}
            target="_blank"
            rel="noopener noreferrer"
          >
            {t("online.console")}
          </a>
          <Button
            size="sm"
            variant="ghost"
            onPress={() =>
              void run(async () => {
                await navigator.clipboard.writeText(address || new URL(openHref, location.origin).href);
                setMessage(t("common.copySuccess"));
              })
            }
          >
            {t("lease.copy")}
          </Button>
        </>
      )}
      {active && (
        <>
          {upgradeTarget && <ClientUpgradeControl installationId={id} leased upgradeTarget={upgradeTarget} canInspect available />}
          {exportButton}
        </>
      )}
      {canEnd && (
        <Button
          size="sm"
          variant="outline"
          isDisabled={busy}
          onPress={() => {
            reset();
            setEnding(true);
          }}
        >
          {t("online.end")}
        </Button>
      )}
      {!ending && status}
      <Modal.Backdrop
        isOpen={ending}
        onOpenChange={(v) => {
          if (!busy) setEnding(v);
        }}
      >
        <Modal.Container placement="center">
          <Modal.Dialog className="w-[min(460px,calc(100vw-2rem))]">
            <Modal.Header>
              <Modal.Heading>{t("online.end")}</Modal.Heading>
            </Modal.Header>
            <Modal.Body className="grid gap-3">
              <p>{t("online.endNotice")}</p>
              {active && exportButton}
              {preview && (
                <>
                  <p>
                    {t("clientTermination.compensation")} ·{" "}
                    {formatWalletUnits(preview.preview.compensationUnits, "$", locale)}
                  </p>
                  <p>
                    {t("clientTermination.return")} ·{" "}
                    {formatWalletUnits(preview.preview.returnUnits, "$", locale)}
                  </p>
                  <div className="grid gap-1.5">
                    <span className="text-xs font-medium text-muted-foreground">{t("clientTermination.reason")}</span>
                    <CompactSelect
                      value={reason}
                      ariaLabel={t("clientTermination.reason")}
                      disabled={busy}
                      triggerClassName="min-h-11 text-sm"
                      onChange={(value) => setReason(value as typeof reason)}
                      options={[
                        { value: "voluntary", label: t("clientTermination.voluntary") },
                        { value: "provider_issue", label: t("clientTermination.providerIssue") },
                        { value: "platform_issue", label: t("clientTermination.platformIssue") },
                      ]}
                    />
                  </div>
                  {reason !== "voluntary" && (
                    <label className="grid gap-1.5">
                      <span className="text-xs font-medium text-muted-foreground">{t("clientTermination.claim")}</span>
                      <TextArea
                        value={claim}
                        rows={3}
                        disabled={busy}
                        onChange={(e) => setClaim(e.target.value)}
                        maxLength={2000}
                      />
                    </label>
                  )}
                </>
              )}
              {status}
            </Modal.Body>
            <Modal.Footer>
              <Button variant="ghost" isDisabled={busy} onPress={() => setEnding(false)}>
                {t("common.cancel")}
              </Button>
              <Button
                isDisabled={busy || (reason !== "voluntary" && !terminationExplanationValid(claim))}
                onPress={() =>
                  void run(async () => {
                    if (!preview || terminationPreviewExpired(preview)) {
                      setPreview(await previewClientTermination(id));
                      return;
                    }
                    try {
                      await onlineRequest("leases/" + encodeURIComponent(id) + "/end", "POST", {
                        previewId: preview.preview.previewId,
                        reason,
                        claim: reason === "voluntary" ? "" : claim,
                      });
                    } catch (e) {
                      if (e instanceof ApiError && e.status === 409)
                        setPreview(await previewClientTermination(id));
                      throw e;
                    }
                    setEnding(false);
                    reset();
                    onChanged();
                  })
                }
              >
                {t(preview ? "online.end" : "online.previewExit")}
              </Button>
            </Modal.Footer>
          </Modal.Dialog>
        </Modal.Container>
      </Modal.Backdrop>
    </div>
  );
}
