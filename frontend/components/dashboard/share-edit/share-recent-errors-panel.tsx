"use client";

import { Alert, Button } from "@heroui/react";
import { Loader2, RefreshCw } from "lucide-react";
import * as React from "react";
import { getShareRecentErrors } from "@/lib/api";
import type { ShareRecentError } from "@/lib/types";
import { formatDateTime } from "@/lib/utils";
import type { TFn } from "@/components/dashboard/share-dashboard-utils";
import { EmptyBlock } from "@/components/dashboard/drawer-panels";
import { ShareEditSection } from "./share-edit-section";

function captureReasonLabel(reason: string, t: TFn) {
  switch (reason) {
    case "buffered":
      return t("dashboard.shareRecentErrors.reason.buffered");
    case "sse_not_buffered":
      return t("dashboard.shareRecentErrors.reason.sse_not_buffered");
    case "empty_body":
      return t("dashboard.shareRecentErrors.reason.empty_body");
    case "read_failed":
      return t("dashboard.shareRecentErrors.reason.read_failed");
    case "router_local":
      return t("dashboard.shareRecentErrors.reason.router_local");
    default:
      return reason;
  }
}

export function ShareRecentErrorsPanel({
  shareId,
  t,
}: {
  shareId: string;
  t: TFn;
}) {
  const [items, setItems] = React.useState<ShareRecentError[]>([]);
  const [loading, setLoading] = React.useState(true);
  const [error, setError] = React.useState("");

  const load = React.useCallback(
    async (signal?: AbortSignal) => {
      setLoading(true);
      try {
        const page = await getShareRecentErrors(shareId, signal);
        setItems(page.errors || []);
        setError("");
      } catch (reason) {
        if (signal?.aborted) return;
        setError(reason instanceof Error ? reason.message : String(reason));
      } finally {
        if (!signal?.aborted) setLoading(false);
      }
    },
    [shareId],
  );

  React.useEffect(() => {
    const controller = new AbortController();
    setItems([]);
    setError("");
    void load(controller.signal);
    return () => controller.abort();
  }, [load]);

  return (
    <ShareEditSection title={t("dashboard.shareRecentErrors.title")}>
      <div className="flex items-start justify-between gap-3">
        <p className="text-xs leading-5 text-slate-500">
          {t("dashboard.shareRecentErrors.description")}
        </p>
        <Button
          size="sm"
          variant="ghost"
          isIconOnly
          aria-label={t("dashboard.shareRecentErrors.refresh")}
          onClick={() => void load()}
          isDisabled={loading}
        >
          <RefreshCw className={`h-4 w-4 ${loading ? "animate-spin" : ""}`} />
        </Button>
      </div>

      {error ? <Alert status="danger">{error}</Alert> : null}

      {loading && items.length === 0 ? (
        <EmptyBlock>
          <span className="inline-flex items-center gap-2">
            <Loader2 className="h-4 w-4 animate-spin" />
            {t("dashboard.shareRecentErrors.loading")}
          </span>
        </EmptyBlock>
      ) : items.length === 0 ? (
        <EmptyBlock>{t("dashboard.shareRecentErrors.empty")}</EmptyBlock>
      ) : (
        <div className="overflow-x-auto rounded-xl border border-slate-200">
          <table className="w-full min-w-[36rem] border-collapse text-left text-xs">
            <thead className="bg-slate-50 text-[11px] uppercase tracking-wide text-slate-500">
              <tr>
                <th className="px-3 py-2 font-medium">{t("dashboard.shareRecentErrors.request")}</th>
                <th className="px-3 py-2 font-medium">{t("dashboard.shareRecentErrors.caller")}</th>
                <th className="px-3 py-2 font-medium">{t("dashboard.shareRecentErrors.time")}</th>
              </tr>
            </thead>
            <tbody>
              {items.map((item) => (
                <tr key={item.id} className="border-t border-slate-200 align-top">
                  <td className="px-3 py-3">
                    <div className="font-mono text-sm font-semibold text-slate-900">
                      {t("dashboard.shareRecentErrors.status", { code: item.statusCode })}
                    </div>
                    <div className="mt-1 break-all text-[11px] text-slate-500">
                      {[item.method, item.path].filter(Boolean).join(" ") || "—"}
                    </div>
                    <div className="mt-1 text-[11px] text-slate-500">
                      {captureReasonLabel(item.bodyCaptureReason, t)}
                      {item.bodyTruncated
                        ? ` · ${t("dashboard.shareRecentErrors.truncated")}`
                        : ""}
                    </div>
                    <pre className="mt-2 max-h-48 overflow-auto whitespace-pre-wrap break-all rounded-lg bg-slate-950 px-2.5 py-2 font-mono text-[11px] leading-5 text-slate-100">
                      {item.bodyText || " "}
                    </pre>
                  </td>
                  <td className="px-3 py-3 font-mono text-slate-700">
                    {item.callerEmail || t("dashboard.shareRecentErrors.anonymous")}
                  </td>
                  <td className="px-3 py-3 whitespace-nowrap text-slate-500">
                    {formatDateTime(item.capturedAt)}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </ShareEditSection>
  );
}
