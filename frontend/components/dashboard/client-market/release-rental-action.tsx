"use client";

import * as React from "react";
import { Button, Chip, Modal, toast } from "@heroui/react";
import { Loader2, Unlink } from "lucide-react";
import { ConfirmAlertDialog } from "@/components/common/confirm-alert-dialog";
import { ProvisionJobLog } from "@/components/dashboard/provision-job-log";
import { useLocaleText } from "@/components/i18n/locale-provider";
import {
  finalizeClientMarketFailedRental,
  getClientMarketJob,
  releaseClientMarketRental,
} from "@/lib/api";
import {
  cleanupFailureGuidanceKey,
  cleanupPhaseLabelKey,
} from "@/components/dashboard/client-market/host-utils";
import type { ClientMarketRental, ProvisioningJob } from "@/lib/types";
import { getClientTermination, previewClientTermination, terminationExplanationValid, type ClientTermination, type ClientTerminationPreview, type ClientTerminationRequest } from "@/lib/client-termination";
import { formatWalletUnits } from "@/lib/market-wallet-money";
import { notifyMarketBalanceChanged } from "@/lib/market-balance-events";
import { useAuth } from "@/components/auth/auth-provider";
import { WALLET_INPUT } from "@/components/market/wallet-primitives";

const POLL_INTERVAL_MS = 1200;
const POLL_MAX_ATTEMPTS = 180;
const RESUME_STORAGE_PREFIX = "client-market:release-job:";

function resumeStorageKey(installationId: string) {
  return `${RESUME_STORAGE_PREFIX}${installationId}`;
}

export function FinalizeFailedRentalAction({
  rental,
  onChanged,
  className,
}: {
  rental: ClientMarketRental;
  onChanged: () => Promise<void> | void;
  className?: string;
}) {
  const { t } = useLocaleText();
  const [confirmOpen, setConfirmOpen] = React.useState(false);
  const [busy, setBusy] = React.useState(false);

  if (!rental.canFinalizeRelease) return null;

  const finalize = async () => {
    setBusy(true);
    try {
      await finalizeClientMarketFailedRental(rental.installationId);
      setConfirmOpen(false);
      toast.success(t("clientMarket.release.finalizeSucceeded"));
      await onChanged();
    } catch (reason) {
      toast.danger(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
    }
  };

  return (
    <>
      <Button
        size="sm"
        variant="ghost"
        className={className ?? "h-6 px-2 text-[11px] text-amber-800"}
        isDisabled={busy}
        onClick={() => setConfirmOpen(true)}
      >
        <Unlink className="h-3.5 w-3.5" />
        {t("clientMarket.release.finalizeAction")}
      </Button>
      <ConfirmAlertDialog
        open={confirmOpen}
        title={t("clientMarket.release.finalizeConfirmTitle")}
        description={t("clientMarket.release.finalizeConfirmDescription")}
        confirmLabel={t("clientMarket.release.finalizeAction")}
        cancelLabel={t("common.cancel")}
        busy={busy}
        tone="danger"
        onConfirm={() => void finalize()}
        onOpenChange={(next) => !busy && setConfirmOpen(next)}
      />
    </>
  );
}

export function ReleaseRentalAction({
  rental,
  onChanged,
  label,
  className,
}: {
  rental: ClientMarketRental;
  onChanged: () => Promise<void> | void;
  /** Overrides the button text, e.g. "Retry release" on a failed release. */
  label?: string;
  className?: string;
}) {
  const { locale, t } = useLocaleText();
  const { session } = useAuth();
  const actor = session?.user?.id || session?.user?.email;
  const [confirmOpen, setConfirmOpen] = React.useState(false);
  const [busy, setBusy] = React.useState(false);
  const [job, setJob] = React.useState<ProvisioningJob | null>(null);
  const [progressOpen, setProgressOpen] = React.useState(false);
  const [preview, setPreview] = React.useState<ClientTerminationPreview | null>(null);
  const [termination, setTermination] = React.useState<ClientTermination | null>(null);
  const [previewError, setPreviewError] = React.useState("");
  const [previewAttempt, setPreviewAttempt] = React.useState(0);
  const [reason, setReason] = React.useState<ClientTerminationRequest["reason"]>("voluntary");
  const [claim, setClaim] = React.useState("");
  const cancelledRef = React.useRef(false);
  const trackingJobIdRef = React.useRef<string | null>(null);

  const refreshSettlement = React.useCallback(async () => {
    const result = await getClientTermination(rental.installationId);
    if (!cancelledRef.current) setTermination(result);
    if (actor) notifyMarketBalanceChanged(actor);
  }, [actor, rental.installationId]);
  React.useEffect(() => {
    if (!confirmOpen) return;
    let active = true;
    setPreview(null); setPreviewError("");
    previewClientTermination(rental.installationId).then(value => { if (active) setPreview(value); })
      .catch(error => { if (active) setPreviewError(error instanceof Error ? error.message : String(error)); });
    return () => { active = false; };
  }, [confirmOpen, rental.installationId, previewAttempt]);

  React.useEffect(() => {
    cancelledRef.current = false;
    return () => { cancelledRef.current = true; };
  }, []);

  const pollJob = React.useCallback(
    async (jobId: string) => {
      trackingJobIdRef.current = jobId;
      for (let i = 0; i < POLL_MAX_ATTEMPTS; i++) {
        await new Promise((resolve) => setTimeout(resolve, POLL_INTERVAL_MS));
        if (cancelledRef.current || trackingJobIdRef.current !== jobId) return;
        let latest: ProvisioningJob;
        try {
          latest = await getClientMarketJob(jobId);
        } catch {
          continue;
        }
        if (cancelledRef.current || trackingJobIdRef.current !== jobId) return;
        setJob(latest);
        if (latest.status === "succeeded") {
          await refreshSettlement();
          try {
            sessionStorage.removeItem(resumeStorageKey(rental.installationId));
          } catch {
            /* ignore */
          }
          toast.success(t("clientMarket.release.succeeded"));
          await onChanged();
          return;
        }
        if (latest.status === "failed") {
          try {
            sessionStorage.removeItem(resumeStorageKey(rental.installationId));
          } catch {
            /* ignore */
          }
          toast.danger(t(cleanupFailureGuidanceKey(latest.failureCode || latest.log)));
          await onChanged();
          return;
        }
      }
      toast.danger(t("clientMarket.cleanupTimedOut"));
      await onChanged();
    },
    [rental.installationId, onChanged, refreshSettlement, t],
  );

  const attachToJob = React.useCallback(
    async (jobId: string) => {
      setBusy(true);
      setProgressOpen(true);
      trackingJobIdRef.current = jobId;
      try {
        try {
          const latest = await getClientMarketJob(jobId);
          if (cancelledRef.current || trackingJobIdRef.current !== jobId) return;
          setJob(latest);
          if (latest.status === "succeeded") {
            await refreshSettlement();
            try {
              sessionStorage.removeItem(resumeStorageKey(rental.installationId));
            } catch {
              /* ignore */
            }
            toast.success(t("clientMarket.release.succeeded"));
            await onChanged();
            return;
          }
          if (latest.status === "failed") {
            try {
              sessionStorage.removeItem(resumeStorageKey(rental.installationId));
            } catch {
              /* ignore */
            }
            toast.danger(t(cleanupFailureGuidanceKey(latest.failureCode || latest.log)));
            await onChanged();
            return;
          }
        } catch {
          /* fall through to polling */
        }
        await pollJob(jobId);
      } finally {
        if (trackingJobIdRef.current === jobId) setBusy(false);
      }
    },
    [rental.installationId, onChanged, pollJob, refreshSettlement, t],
  );

  // Reattach after refresh: the rental record is authoritative; sessionStorage
  // covers the brief window before the next rental poll returns the job id.
  React.useEffect(() => {
    if (cancelledRef.current) return;
    let stored: string | null = null;
    try {
      stored = sessionStorage.getItem(resumeStorageKey(rental.installationId));
    } catch {
      stored = null;
    }
    const jobId =
      rental.activeCleanupJobId ||
      (rental.status === "releasing" ? stored : null) ||
      null;
    if (!jobId) return;
    if (trackingJobIdRef.current === jobId) return;
    void attachToJob(jobId);
  }, [
    attachToJob,
    rental.activeCleanupJobId,
    rental.installationId,
    rental.status,
  ]);

  const release = React.useCallback(async () => {
    if (!preview) return;
    setBusy(true);
    try {
      const response = await releaseClientMarketRental(rental.installationId, { previewId: preview.previewId, reason, claim: reason === "voluntary" ? "" : claim });
      const jobId = response.jobId;
      if (!jobId) { await refreshSettlement(); setConfirmOpen(false); setProgressOpen(true); await onChanged(); return; }
      try {
        sessionStorage.setItem(resumeStorageKey(rental.installationId), jobId);
      } catch {
        /* ignore quota / private mode */
      }
      // Claim the job before onChanged so the resume effect does not start a second poller.
      trackingJobIdRef.current = jobId;
      setConfirmOpen(false);
      setJob(null);
      setProgressOpen(true);
      await onChanged();
      await pollJob(jobId);
    } catch (reason) {
      toast.danger(reason instanceof Error ? reason.message : String(reason));
      setPreviewAttempt(n => n + 1);
      await onChanged();
    } finally {
      setBusy(false);
    }
  }, [rental.installationId, onChanged, pollJob, preview, reason, claim, refreshSettlement]);

  const jobPhase: "running" | "failed" | "success" =
    job?.status === "failed" ? "failed" : job?.status === "succeeded" ? "success" : "running";
  const settled = job?.status === "failed" || job?.status === "succeeded" || !!termination?.cleanupConfirmedAt;
  const showReleaseButton = rental.canRelease
    && rental.status !== "releasing";

  return (
    <>
      {showReleaseButton ? (
        <Button
          size="sm"
          variant="ghost"
          className={className ?? "h-7 px-2 text-[11px] text-rose-700"}
          isDisabled={busy}
          onClick={() => setConfirmOpen(true)}
        >
          {label ?? t("clientTermination.immediate")}
        </Button>
      ) : null}

      <ConfirmAlertDialog
        open={confirmOpen}
        title={t("clientTermination.confirmTitle")}
        description={t("clientTermination.confirmHelp")}
        confirmLabel={t("clientTermination.immediate")}
        cancelLabel={t("common.cancel")}
        busy={busy}
        confirmDisabled={!preview || (reason !== "voluntary" && !terminationExplanationValid(claim))}
        tone="danger"
        extra={
          <div className="grid max-h-[55vh] gap-3 overflow-y-auto">
            <p className="rounded-md bg-amber-50 p-3 text-xs leading-5 text-amber-950">{t("clientTermination.dataWarning")}</p>
            {previewError ? <p role="alert" className="text-danger">{previewError}</p> : null}
            {!preview ? <Button variant="ghost" onPress={() => setPreviewAttempt(n => n + 1)}>{t(previewError ? "common.retry" : "common.loading")}</Button> : <>
              <dl className="grid grid-cols-2 gap-x-4 gap-y-2 rounded-lg bg-slate-50 p-3 text-xs tabular-nums">
                {([
                  ["clientTermination.period", preview.periodAmountUnits],
                  ["clientTermination.remaining", preview.remainingUnits],
                  [reason === "voluntary" ? "clientTermination.compensation" : "clientTermination.held", preview.compensationUnits],
                  ["clientTermination.return", preview.returnUnits],
                ] as const).map(([key, amount]) => <React.Fragment key={key}><dt>{t(key)}</dt><dd className="text-right font-medium">{formatWalletUnits(amount, "$", locale)}</dd></React.Fragment>)}
              </dl>
              <p className="text-xs text-muted-foreground">{preview.policyVersion === 0 ? t("clientTermination.noRetroactive") : t("clientTermination.purchaseTerms", { rate: String(preview.rateBps / 100) })}</p>
              {preview.nextRentalTrial ? <p className="text-xs leading-5 text-muted-foreground">{t("clientTrial.releasePaid")}</p>
                : preview.trialSecondsRemaining != null ? <p className="text-xs leading-5 text-muted-foreground">{t("clientTrial.releaseUnpaid")}</p> : null}
              <label className="grid gap-1 text-sm">{t("clientTermination.reason")}<select className={WALLET_INPUT} value={reason} onChange={e => setReason(e.target.value as ClientTerminationRequest["reason"])}>
                <option value="voluntary">{t("clientTermination.voluntary")}</option><option value="provider_issue">{t("clientTermination.providerIssue")}</option><option value="platform_issue">{t("clientTermination.platformIssue")}</option>
              </select></label>
              {reason !== "voluntary" ? <><label className="grid gap-1 text-xs">{t("clientTermination.claim")}<textarea className={WALLET_INPUT} value={claim} maxLength={2000} onChange={e => setClaim(e.target.value)} /></label>{!terminationExplanationValid(claim) ? <p role="alert" className="text-xs text-danger">{t("clientTermination.explanationTooLong")}</p> : null}<p className="text-xs text-muted-foreground">{t("clientTermination.claimHelp")}</p><p className="text-xs text-muted-foreground">{t("clientTermination.chatHelp")}</p><a href={`/clients/?chat=${encodeURIComponent(rental.installationId)}`} target="_blank" rel="noopener noreferrer" className="text-xs underline">{t("clientTermination.chat")}</a></> : null}
              <p className="text-xs text-muted-foreground">{t("clientTermination.expires")} <button type="button" className="underline" onClick={() => setPreviewAttempt(n => n + 1)}>{t("common.reload")}</button></p>
            </>}
          </div>
        }
        onConfirm={() => void release()}
        onOpenChange={(next) => !busy && setConfirmOpen(next)}
      />

      <Modal.Backdrop
        isOpen={progressOpen}
        onOpenChange={(next) => {
          // Closing mid-teardown would hide the only progress the renter gets.
          if (!next && settled) setProgressOpen(false);
        }}
      >
        <Modal.Container placement="center">
          <Modal.Dialog className="light w-[min(640px,calc(100vw-2rem))] max-w-none !bg-white !text-slate-900">
            <Modal.Header>
              <Modal.Heading>{t("clientMarket.release.progressTitle")}</Modal.Heading>
            </Modal.Header>
            <Modal.Body className="grid max-h-[70vh] grid-cols-[minmax(0,1fr)] gap-3 overflow-y-auto">
              <div className="flex flex-wrap items-center gap-2">
                <Chip
                  size="sm"
                  variant="soft"
                  className={
                    jobPhase === "failed"
                      ? "bg-rose-100 text-rose-700"
                      : jobPhase === "success"
                        ? "bg-emerald-100 text-emerald-700"
                        : undefined
                  }
                >
                  {job ? t(cleanupPhaseLabelKey(job.phase || "")) : t("clientMarket.release.starting")}
                </Chip>
                {job?.status ? (
                  <span className="text-xs text-muted-foreground">{job.status}</span>
                ) : null}
                {!settled ? <Loader2 className="h-3.5 w-3.5 animate-spin text-muted-foreground" /> : null}
              </div>

              <ProvisionJobLog log={job?.log || ""} phase={jobPhase} intent="release" />

              {job?.status === "failed" ? (
                <p className="text-xs leading-5 text-rose-700">
                  {t(cleanupFailureGuidanceKey(job.failureCode || job.log))}
                </p>
              ) : null}
              {job?.status === "succeeded" ? (
                <p className="text-xs leading-5 text-emerald-700">{t("clientMarket.release.succeeded")}</p>
              ) : null}
              {termination ? <div className="grid gap-1 text-xs"><p>{t(`clientTermination.${termination.status}`)}</p><p>{t("clientTermination.return")}: {formatWalletUnits(termination.returnedUnits, "$", locale)}</p><p>{t("clientTermination.compensation")}: {formatWalletUnits(termination.compensationUnits, "$", locale)}</p><p>{t("clientTermination.held")}: {formatWalletUnits(termination.heldUnits, "$", locale)}</p></div> : null}
              {(reason !== "voluntary" || termination?.disputeUsed) ? <div className="grid gap-2 rounded-lg bg-amber-50 p-3 text-xs leading-5"><p>{t("clientTermination.claimHelp")}</p><p>{t("clientTermination.chatHelp")}</p><a href={`/clients/?chat=${encodeURIComponent(rental.installationId)}`} target="_blank" rel="noopener noreferrer" className="underline">{t("clientTermination.chat")}</a></div> : null}
              <a href="/account/wallet/" className="text-xs underline">{t("clientTermination.history")}</a>
            </Modal.Body>
            <Modal.Footer>
              <Button variant="ghost" isDisabled={!settled} onClick={() => setProgressOpen(false)}>
                {t("common.close")}
              </Button>
            </Modal.Footer>
          </Modal.Dialog>
        </Modal.Container>
      </Modal.Backdrop>
    </>
  );
}
