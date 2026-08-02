"use client";

import { Button, Checkbox, Modal, toast } from "@heroui/react";
import { AlertTriangle, CheckCircle2, Fingerprint, Loader2 } from "lucide-react";
import * as React from "react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import {
  ApiError,
  rotateClientMarketHostSshKey,
  scanClientMarketHostSshKey,
} from "@/lib/api";
import type {
  ClientMarketSshHostKeyInspection,
  ClientMarketSshHostKeyRotationResponse,
} from "@/lib/types";

const HOST_KEY_ERROR_CODES = new Set([
  "SSH_HOST_KEY_CONFIRMATION_REQUIRED",
  "SSH_HOST_KEY_CHANGED_DURING_CONFIRMATION",
]);

export function sshHostKeyInspectionFromError(
  error: unknown,
): ClientMarketSshHostKeyInspection | null {
  if (!(error instanceof ApiError) || !error.code || !HOST_KEY_ERROR_CODES.has(error.code)) {
    return null;
  }
  const details = error.details;
  if (
    !details ||
    typeof details.hostId !== "string" ||
    typeof details.endpoint !== "string" ||
    typeof details.observedFingerprint !== "string" ||
    typeof details.observedKeyType !== "string" ||
    typeof details.changed !== "boolean" ||
    typeof details.confirmationRequired !== "boolean"
  ) {
    return null;
  }
  return {
    hostId: details.hostId,
    endpoint: details.endpoint,
    storedFingerprint:
      typeof details.storedFingerprint === "string" ? details.storedFingerprint : undefined,
    observedFingerprint: details.observedFingerprint,
    observedKeyType: details.observedKeyType,
    changed: details.changed,
    confirmationRequired: details.confirmationRequired,
  };
}

function hostKeyPublicPath(keyType: string) {
  if (keyType === "ssh-ed25519") return "/etc/ssh/ssh_host_ed25519_key.pub";
  if (keyType.startsWith("ecdsa-sha2-")) return "/etc/ssh/ssh_host_ecdsa_key.pub";
  return "/etc/ssh/ssh_host_rsa_key.pub";
}

function FingerprintValue({ value, muted = false }: { value?: string; muted?: boolean }) {
  return (
    <span
      className={`min-w-0 break-all font-mono text-xs leading-5 ${muted ? "text-muted-foreground" : "text-slate-900"}`}
    >
      {value || "-"}
    </span>
  );
}

export function HostKeyRotationDialog({
  hostId,
  hostLabel,
  open,
  initialInspection,
  onOpenChange,
  onUpdated,
}: {
  hostId: string;
  hostLabel: string;
  open: boolean;
  initialInspection?: ClientMarketSshHostKeyInspection | null;
  onOpenChange: (open: boolean) => void;
  onUpdated?: (response: ClientMarketSshHostKeyRotationResponse) => void;
}) {
  const { t } = useLocaleText();
  const [inspection, setInspection] = React.useState<ClientMarketSshHostKeyInspection | null>(null);
  const [confirmedFingerprint, setConfirmedFingerprint] = React.useState("");
  const [verified, setVerified] = React.useState(false);
  const [loading, setLoading] = React.useState(false);
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState("");
  const [scanGeneration, setScanGeneration] = React.useState(0);

  React.useEffect(() => {
    if (!open) return;
    setConfirmedFingerprint("");
    setVerified(false);
    setError("");
    const controller = new AbortController();
    setInspection(initialInspection?.hostId === hostId ? initialInspection : null);
    setLoading(true);
    void scanClientMarketHostSshKey(hostId, controller.signal)
      .then((result) => setInspection(result))
      .catch((reason) => {
        if (controller.signal.aborted) return;
        setInspection(null);
        setError(reason instanceof Error ? reason.message : String(reason));
      })
      .finally(() => {
        if (!controller.signal.aborted) setLoading(false);
      });
    return () => controller.abort();
  }, [hostId, initialInspection, open, scanGeneration]);

  const observedFingerprint = inspection?.observedFingerprint || "";
  const fingerprintMatches = confirmedFingerprint.trim() === observedFingerprint;
  const canRotate =
    !!inspection?.confirmationRequired && fingerprintMatches && verified && !loading && !busy;

  const rotate = async () => {
    if (!inspection || !canRotate) return;
    setBusy(true);
    setError("");
    try {
      const response = await rotateClientMarketHostSshKey(hostId, {
        expectedCurrentFingerprint: inspection.storedFingerprint,
        confirmedFingerprint: confirmedFingerprint.trim(),
        verifiedFromHostConsole: verified,
      });
      toast.success(t("clientMarket.sshHostKey.updated"));
      onUpdated?.(response);
      onOpenChange(false);
    } catch (reason) {
      const nextInspection = sshHostKeyInspectionFromError(reason);
      if (nextInspection) {
        setInspection(nextInspection);
        setConfirmedFingerprint("");
        setVerified(false);
        setError(t("clientMarket.sshHostKey.changedDuringConfirmation"));
      } else {
        setError(reason instanceof Error ? reason.message : String(reason));
      }
    } finally {
      setBusy(false);
    }
  };

  const close = (nextOpen: boolean) => {
    if (!busy) onOpenChange(nextOpen);
  };

  return (
    <Modal.Backdrop isOpen={open} onOpenChange={close}>
      <Modal.Container placement="center">
        <Modal.Dialog className="light w-[min(620px,calc(100vw-2rem))] max-w-none !bg-white !text-slate-900">
          <Modal.Header>
            <Modal.Heading className="inline-flex items-center gap-2">
              <Fingerprint className="h-5 w-5 text-slate-600" />
              {t("clientMarket.sshHostKey.title", { host: hostLabel })}
            </Modal.Heading>
          </Modal.Header>
          <Modal.Body className="grid gap-4">
            {loading ? (
              <div className="flex min-h-32 items-center justify-center gap-2 text-sm text-muted-foreground">
                <Loader2 className="h-4 w-4 animate-spin text-primary" />
                {t("clientMarket.sshHostKey.scanning")}
              </div>
            ) : null}

            {!loading && inspection ? (
              <>
                <div className="grid gap-2 text-sm">
                  <div className="grid gap-1 border-b border-slate-100 pb-2 sm:grid-cols-[9rem_minmax(0,1fr)]">
                    <span className="text-muted-foreground">{t("clientMarket.sshHostKey.endpoint")}</span>
                    <FingerprintValue value={inspection.endpoint} />
                  </div>
                  <div className="grid gap-1 border-b border-slate-100 pb-2 sm:grid-cols-[9rem_minmax(0,1fr)]">
                    <span className="text-muted-foreground">{t("clientMarket.sshHostKey.stored")}</span>
                    <FingerprintValue
                      value={inspection.storedFingerprint || t("clientMarket.sshHostKey.notPinned")}
                      muted={!inspection.storedFingerprint}
                    />
                  </div>
                  <div className="grid gap-1 border-b border-slate-100 pb-2 sm:grid-cols-[9rem_minmax(0,1fr)]">
                    <span className="text-muted-foreground">{t("clientMarket.sshHostKey.observed")}</span>
                    <FingerprintValue value={inspection.observedFingerprint} />
                  </div>
                  <div className="grid gap-1 sm:grid-cols-[9rem_minmax(0,1fr)]">
                    <span className="text-muted-foreground">{t("clientMarket.sshHostKey.algorithm")}</span>
                    <FingerprintValue value={inspection.observedKeyType} muted />
                  </div>
                </div>

                {!inspection.confirmationRequired ? (
                  <div className="flex items-start gap-2 rounded-md border border-emerald-200 bg-emerald-50 px-3 py-2 text-sm leading-5 text-emerald-900">
                    <CheckCircle2 className="mt-0.5 h-4 w-4 shrink-0" />
                    <span>{t("clientMarket.sshHostKey.matches")}</span>
                  </div>
                ) : (
                  <div className="grid gap-3">
                    <div className="flex items-start gap-2 rounded-md border border-amber-200 bg-amber-50 px-3 py-2 text-sm leading-5 text-amber-950">
                      <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0" />
                      <span>
                        {t(
                          inspection.changed
                            ? "clientMarket.sshHostKey.changedWarning"
                            : "clientMarket.sshHostKey.firstPinWarning",
                        )}
                      </span>
                    </div>
                    <p className="text-sm leading-6 text-muted-foreground">
                      {t("clientMarket.sshHostKey.consoleInstruction")}
                    </p>
                    <code className="block overflow-x-auto rounded-md border border-slate-200 bg-slate-50 px-3 py-2 font-mono text-xs text-slate-800">
                      ssh-keygen -lf {hostKeyPublicPath(inspection.observedKeyType)}
                    </code>
                    <label className="grid gap-1.5 text-sm">
                      <span className="font-medium text-slate-800">
                        {t("clientMarket.sshHostKey.confirmedFingerprint")}
                      </span>
                      <input
                        value={confirmedFingerprint}
                        onChange={(event) => setConfirmedFingerprint(event.target.value)}
                        placeholder="SHA256:..."
                        spellCheck={false}
                        autoComplete="off"
                        className="h-10 min-w-0 rounded-md border border-slate-300 px-3 font-mono text-xs outline-none focus:border-primary focus:ring-2 focus:ring-primary/20"
                      />
                      {confirmedFingerprint && !fingerprintMatches ? (
                        <span className="text-xs text-rose-600">
                          {t("clientMarket.sshHostKey.fingerprintMismatch")}
                        </span>
                      ) : null}
                    </label>
                    <Checkbox
                      isSelected={verified}
                      onChange={setVerified}
                      isDisabled={busy}
                      className="items-start"
                    >
                      <Checkbox.Control className="mt-0.5 border border-slate-300 bg-white shadow-none">
                        <Checkbox.Indicator />
                      </Checkbox.Control>
                      <Checkbox.Content className="text-sm leading-5 text-slate-700">
                        {t("clientMarket.sshHostKey.verifiedFromConsole")}
                      </Checkbox.Content>
                    </Checkbox>
                  </div>
                )}
              </>
            ) : null}

            {!loading && error ? (
              <div className="grid gap-2 rounded-md border border-rose-200 bg-rose-50 px-3 py-2 text-sm text-rose-700">
                <span>{error}</span>
                {!inspection ? (
                  <Button
                    variant="outline"
                    size="sm"
                    className="w-fit"
                    onClick={() => setScanGeneration((value) => value + 1)}
                  >
                    {t("common.retry")}
                  </Button>
                ) : null}
              </div>
            ) : null}
          </Modal.Body>
          <Modal.Footer>
            <Button variant="ghost" isDisabled={busy} onClick={() => close(false)}>
              {inspection?.confirmationRequired ? t("common.cancel") : t("common.close")}
            </Button>
            {inspection?.confirmationRequired ? (
              <Button variant="primary" isDisabled={!canRotate} onClick={() => void rotate()}>
                {busy ? <Loader2 className="h-4 w-4 animate-spin" /> : <Fingerprint className="h-4 w-4" />}
                {t("clientMarket.sshHostKey.confirm")}
              </Button>
            ) : null}
          </Modal.Footer>
        </Modal.Dialog>
      </Modal.Container>
    </Modal.Backdrop>
  );
}
