"use client";

import * as React from "react";
import { Button, Chip, Modal, toast } from "@heroui/react";
import { Loader2 } from "lucide-react";
import { ConfirmAlertDialog } from "@/components/common/confirm-alert-dialog";
import { ProvisionJobLog } from "@/components/dashboard/provision-job-log";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { cleanupClientMarketClientWithReason, getClientMarketJob } from "@/lib/api";
import {
  cleanupFailureGuidanceKey,
  cleanupPhaseLabelKey,
} from "@/components/dashboard/client-market/host-utils";
import type { ClientMarketBilling, ProvisioningJob } from "@/lib/types";

const POLL_INTERVAL_MS = 1200;
const POLL_MAX_ATTEMPTS = 180;

/**
 * Renter-initiated release of a rented Client.
 *
 * Three things this fixes over the previous behaviour:
 *
 * 1. **Always reachable.** Release used to live only inside the payment modal,
 *    which is itself gated on `billingUrgencyTier` being urgent. A paid rental with
 *    time left — and any free rental, where the tier is `null` — rendered no banner
 *    at all, so the renter had no way to give the Host back.
 * 2. **States the refund policy.** Releasing early does not refund the current
 *    period; the confirm dialog now says so instead of leaving the renter to guess.
 * 3. **Shows the teardown.** Release kicks off a remote cleanup job. Previously the
 *    renter only got a fire-and-forget toast; now the job's phases and log stream
 *    into a progress dialog, the same view the Host owner already gets.
 */
export function ReleaseRentalAction({
  billing,
  onChanged,
  label,
  className,
}: {
  billing: ClientMarketBilling;
  onChanged: () => Promise<void> | void;
  /** Overrides the button text, e.g. "Retry release" on a failed release. */
  label?: string;
  className?: string;
}) {
  const { t } = useLocaleText();
  const [confirmOpen, setConfirmOpen] = React.useState(false);
  const [busy, setBusy] = React.useState(false);
  const [job, setJob] = React.useState<ProvisioningJob | null>(null);
  const [progressOpen, setProgressOpen] = React.useState(false);
  const cancelledRef = React.useRef(false);

  React.useEffect(
    () => () => {
      cancelledRef.current = true;
    },
    [],
  );

  const pollJob = React.useCallback(
    async (jobId: string) => {
      for (let i = 0; i < POLL_MAX_ATTEMPTS; i++) {
        await new Promise((resolve) => setTimeout(resolve, POLL_INTERVAL_MS));
        if (cancelledRef.current) return;
        let latest: ProvisioningJob;
        try {
          latest = await getClientMarketJob(jobId);
        } catch {
          continue;
        }
        if (cancelledRef.current) return;
        setJob(latest);
        if (latest.status === "succeeded") {
          toast.success(t("billing.releaseSucceeded"));
          await onChanged();
          return;
        }
        if (latest.status === "failed") {
          const detail = latest.failureCode || latest.log.split("\n").filter(Boolean).at(-1) || "";
          toast.danger(
            detail ? `${t("billing.releaseFailedToast")}: ${detail}` : t("billing.releaseFailedToast"),
          );
          await onChanged();
          return;
        }
      }
      toast.danger(t("clientMarket.cleanupTimedOut"));
      await onChanged();
    },
    [onChanged, t],
  );

  const release = React.useCallback(async () => {
    setBusy(true);
    try {
      const { jobId } = await cleanupClientMarketClientWithReason(billing.installationId, {
        reason: "client_release",
        blockClientForProvider: false,
      });
      setConfirmOpen(false);
      setJob(null);
      setProgressOpen(true);
      await onChanged();
      await pollJob(jobId);
    } catch (reason) {
      setConfirmOpen(false);
      toast.danger(reason instanceof Error ? reason.message : String(reason));
      await onChanged();
    } finally {
      setBusy(false);
    }
  }, [billing.installationId, onChanged, pollJob]);

  const jobPhase: "running" | "failed" | "success" =
    job?.status === "failed" ? "failed" : job?.status === "succeeded" ? "success" : "running";
  const settled = job?.status === "failed" || job?.status === "succeeded";

  return (
    <>
      <Button
        size="sm"
        variant="ghost"
        className={className ?? "h-7 px-2 text-[11px] text-rose-700"}
        isDisabled={busy}
        onClick={() => setConfirmOpen(true)}
      >
        {label ?? t("billing.releaseClient")}
      </Button>

      <ConfirmAlertDialog
        open={confirmOpen}
        title={t("billing.releaseTitle")}
        // Deliberately spells out that an early release forfeits the paid period.
        description={t("billing.releaseNoRefundDescription")}
        confirmLabel={t("billing.releaseClient")}
        cancelLabel={t("common.cancel")}
        busy={busy}
        tone="danger"
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
              <Modal.Heading>{t("billing.releaseProgressTitle")}</Modal.Heading>
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
                  {job ? t(cleanupPhaseLabelKey(job.phase || "")) : t("billing.releaseStarting")}
                </Chip>
                {job?.status ? (
                  <span className="text-xs text-muted-foreground">{job.status}</span>
                ) : null}
                {!settled ? <Loader2 className="h-3.5 w-3.5 animate-spin text-muted-foreground" /> : null}
              </div>

              <ProvisionJobLog log={job?.log || ""} phase={jobPhase} />

              {job?.status === "failed" ? (
                <p className="text-xs leading-5 text-rose-700">
                  {t(cleanupFailureGuidanceKey(job.failureCode || job.log))}
                </p>
              ) : null}
              {job?.status === "succeeded" ? (
                <p className="text-xs leading-5 text-emerald-700">{t("billing.releaseSucceeded")}</p>
              ) : null}
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
