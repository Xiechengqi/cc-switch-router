"use client";

import * as React from "react";
import { OPERATION_LABELS } from "@/lib/share-capabilities";
import { preciseRequestAmount, type RequestMeteringLoader, type RequestMeteringRow } from "@/lib/share-request-metering";
import { useLoginContextKey } from "@/lib/use-login-context";

export function ShareRequestMetering({
  shareId, requestId, load, zh, login,
}: { shareId: string; requestId: string; load: RequestMeteringLoader; zh: boolean; login?: string }) {
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
      <summary className="cursor-pointer">{zh ? "用量与美元额度" : "Usage and USD quota"}</summary>
      {open && <div className="mt-2 space-y-2">
        {!result ? <p role="status">{zh ? "正在读取…" : "Loading…"}</p> : !result.rows ? (
          <p role="status">{zh ? "无法读取计量记录。" : "Metering could not be loaded."}{" "}
            <button type="button" className="underline" onClick={() => setRetry(n => n + 1)}>{zh ? "重试" : "Retry"}</button>
          </p>
        ) : !result.rows.length ? <p>{zh ? "没有可核对的计量记录。" : "No metering evidence is available."}</p> : result.rows.map(row => {
          const amount = preciseRequestAmount(row.settledPicos);
          const reserve = row.pendingRiskPicos && row.pendingRiskPicos !== "0" ? preciseRequestAmount(row.pendingRiskPicos) : undefined;
          const operations = row.operations.map(op => OPERATION_LABELS[op]?.[zh ? 0 : 1]).filter(Boolean);
          const status = row.chargeMode === "debited"
            ? row.meteringStatus === "pending" ? (zh ? "待结算" : "Pending settlement")
              : amount ? (zh ? "已计入美元额度 " : "Debited from USD quota ") + amount : (zh ? "扣额未知" : "Debit unavailable")
            : row.chargeMode === "estimate" ? (zh ? "仅为估值，未扣美元额度" : "Estimate only. No USD quota debit.")
            : row.chargeMode === "none" ? (zh ? "未使用美元额度" : "No USD quota")
            : (zh ? "无法确认历史扣额" : "Historical debit cannot be confirmed");
          return (
            <div key={row.attempt} className="space-y-1 border-t border-border pt-2">
              <p>{zh ? "尝试" : "Attempt"} {row.attempt} · {status}</p>
              {!!operations.length && <p>{operations.join(zh ? "、" : ", ")}</p>}
              {row.unpricedReason && <p>{zh ? "含未定价用量" : "Contains unpriced usage"}</p>}
              {row.estimated && <p>{zh ? "含估算的资源用量" : "Contains estimated resource quantities"}</p>}
              {reserve && row.chargeMode === "debited" && <p>{zh ? "待结算风险预占 " : "Pending reservation "}{reserve}</p>}
              <details>
                <summary className="cursor-pointer">{zh ? "技术详情" : "Technical details"}</summary>
                <div className="mt-1 space-y-1 break-all text-muted-foreground">
                  <p>{[row.model, row.meteringStatus, row.unpricedReason].filter(Boolean).join(" · ")}</p>
                  {row.chargeMode !== "debited" && amount && <p>{zh ? "记录金额 " : "Recorded amount "}{amount}</p>}
                  {row.dimensions.map(dimension => <p key={dimension.dimension}>{dimension.dimension} · {dimension.quantity}{dimension.pricing === "explicit_free" ? (zh ? " · 明确零价" : " · Explicit zero rate")
                    : preciseRequestAmount(dimension.amountPicos) ? " · " + preciseRequestAmount(dimension.amountPicos) : ""}</p>)}
                  {row.pricingRevision && <p>{zh ? "价目版本 " : "Tariff revision "}{row.pricingRevision}</p>}
                </div>
              </details>
            </div>
          );
        })}
      </div>}
    </details>
  );
}
