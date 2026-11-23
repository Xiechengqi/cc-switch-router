"use client";

import * as React from "react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { OPERATION_LABELS } from "@/lib/share-capabilities";
import { preciseRequestAmount, type RequestMeteringLoader, type RequestMeteringRow } from "@/lib/share-request-metering";
import { useLoginContextKey } from "@/lib/use-login-context";

export function ShareRequestMetering({
  shareId, requestId, load, zh, login,
}: { shareId: string; requestId: string; load: RequestMeteringLoader; zh: boolean; login?: string }) {
  const { t } = useLocaleText();
  const routerLogin = useLoginContextKey();
  const key = JSON.stringify([login ?? routerLogin, shareId, requestId]);
  const [open, setOpen] = React.useState(false);
  const [retry, setRetry] = React.useState(0);
  const [report, setReport] = React.useState<{ key: string; rows?: RequestMeteringRow[] }>();
  const result = report?.key === key ? report : undefined;
  React.useEffect(() => {
    if (!open) return;
    const controller = new AbortController();
    setReport(undefined);
    void load(shareId, requestId, controller.signal)
      .then(rows => { if (!controller.signal.aborted) setReport({ key, rows }); })
      .catch(() => { if (!controller.signal.aborted) setReport({ key }); });
    return () => controller.abort();
  }, [key, load, open, requestId, retry, shareId]);

  return (
    <details className="text-xs" onToggle={event => setOpen(event.currentTarget.open)}>
      <summary className="cursor-pointer">{t("dashboard.metering.title")}</summary>
      {open && <div className="mt-2 space-y-2">
        {!result ? <p role="status">{t("dashboard.metering.loading")}</p> : !result.rows ? (
          <p role="status">{t("dashboard.metering.unavailable")}{" "}
            <button type="button" className="underline" onClick={() => setRetry(n => n + 1)}>{t("dashboard.metering.retry")}</button>
          </p>
        ) : !result.rows.length ? <p>{t("dashboard.metering.empty")}</p> : result.rows.map(row => {
          const amount = preciseRequestAmount(row.settledPicos);
          const reserve = row.pendingRiskPicos && row.pendingRiskPicos !== "0" ? preciseRequestAmount(row.pendingRiskPicos) : undefined;
          const operations = row.operations.map(op => OPERATION_LABELS[op]?.[zh ? 0 : 1]).filter(Boolean);
          const status = row.chargeMode === "debited"
            ? row.meteringStatus === "pending" ? t("dashboard.metering.pending")
              : amount ? t("dashboard.metering.debited", { amount }) : t("dashboard.metering.debitUnknown")
            : row.chargeMode === "estimate" ? t("dashboard.metering.estimate")
            : row.chargeMode === "none" ? t("dashboard.metering.none")
            : t("dashboard.metering.unknown");
          return (
            <div key={row.attempt} className="space-y-1 border-t border-border pt-2">
              <p>{t("dashboard.metering.attempt", { attempt: row.attempt, status })}</p>
              {!!operations.length && <p>{operations.join(t("dashboard.metering.listSeparator"))}</p>}
              {row.unpricedReason && <p>{t("dashboard.metering.unpriced")}</p>}
              {row.estimated && <p>{t("dashboard.metering.estimated")}</p>}
              {reserve && row.chargeMode === "debited" && <p>{t("dashboard.metering.reservation", { amount: reserve })}</p>}
              <details>
                <summary className="cursor-pointer">{t("dashboard.metering.details")}</summary>
                <div className="mt-1 space-y-1 break-all text-muted-foreground">
                  <p>{[row.model, row.meteringStatus, row.unpricedReason].filter(Boolean).join(" · ")}</p>
                  {row.chargeMode !== "debited" && amount && <p>{t("dashboard.metering.recorded", { amount })}</p>}
                  {row.dimensions.map(dimension => <p key={dimension.dimension}>{dimension.dimension} · {dimension.quantity}{dimension.pricing === "explicit_free" ? " · " + t("dashboard.metering.free")
                    : preciseRequestAmount(dimension.amountPicos) ? " · " + preciseRequestAmount(dimension.amountPicos) : ""}</p>)}
                  {row.pricingRevision && <p>{t("dashboard.metering.revision", { revision: row.pricingRevision })}</p>}
                </div>
              </details>
            </div>
          );
        })}
      </div>}
    </details>
  );
}
