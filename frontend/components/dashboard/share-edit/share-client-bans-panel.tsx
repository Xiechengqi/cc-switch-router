"use client";

import { Alert, Button, toast } from "@heroui/react";
import { Loader2, RefreshCw, ShieldBan, Unlock } from "lucide-react";
import * as React from "react";
import { getShareClientBans, unbanShareClient } from "@/lib/api";
import type { ShareClientBan } from "@/lib/types";
import { formatDateTime } from "@/lib/utils";
import type { TFn } from "@/components/dashboard/share-dashboard-utils";
import { EmptyBlock } from "@/components/dashboard/drawer-panels";
import { ShareEditSection } from "./share-edit-section";

function banReasonLabel(reason: string, t: TFn) {
  switch (reason) {
    case "invalid_share_client_credential":
      return t("dashboard.shareClientBans.reason.invalid_share_client_credential");
    case "automated_credential_abuse":
      return t("dashboard.shareClientBans.reason.automated_credential_abuse");
    case "share_policy_abuse":
      return t("dashboard.shareClientBans.reason.share_policy_abuse");
    default:
      return reason;
  }
}

export function ShareClientBansPanel({
  shareId,
  shareName,
  t,
}: {
  shareId: string;
  shareName: string;
  t: TFn;
}) {
  const [items, setItems] = React.useState<ShareClientBan[]>([]);
  const [nextCursor, setNextCursor] = React.useState<string | undefined>();
  const [loading, setLoading] = React.useState(true);
  const [loadingMore, setLoadingMore] = React.useState(false);
  const [error, setError] = React.useState("");
  const [unbanningId, setUnbanningId] = React.useState<string | null>(null);

  const load = React.useCallback(
    async (cursor?: string, signal?: AbortSignal) => {
      if (cursor) setLoadingMore(true);
      else setLoading(true);
      try {
        const page = await getShareClientBans(shareId, cursor, signal);
        setItems((current) => (cursor ? [...current, ...(page.items || [])] : page.items || []));
        setNextCursor(page.nextCursor);
        setError("");
      } catch (reason) {
        if (signal?.aborted) return;
        setError(reason instanceof Error ? reason.message : String(reason));
      } finally {
        if (!signal?.aborted) {
          setLoading(false);
          setLoadingMore(false);
        }
      }
    },
    [shareId],
  );

  React.useEffect(() => {
    const controller = new AbortController();
    setItems([]);
    setNextCursor(undefined);
    setError("");
    void load(undefined, controller.signal);
    return () => controller.abort();
  }, [load]);

  const handleUnban = async (ban: ShareClientBan) => {
    if (
      !window.confirm(
        t("dashboard.shareClientBans.confirm", {
          ip: ban.clientIp,
          share: shareName,
        }),
      )
    ) {
      return;
    }
    setUnbanningId(ban.id);
    try {
      await unbanShareClient(shareId, ban.id);
      setItems((current) => current.filter((item) => item.id !== ban.id));
      toast.success(t("dashboard.shareClientBans.unbanSuccess", { ip: ban.clientIp }));
      setError("");
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setUnbanningId(null);
    }
  };

  return (
    <ShareEditSection title={t("dashboard.shareClientBans.title")}>
      <div className="flex items-start justify-between gap-3">
        <p className="text-xs leading-5 text-slate-500">
          {t("dashboard.shareClientBans.description")}
        </p>
        <Button
          size="sm"
          variant="ghost"
          isIconOnly
          aria-label={t("dashboard.shareClientBans.refresh")}
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
            {t("dashboard.shareClientBans.loading")}
          </span>
        </EmptyBlock>
      ) : items.length === 0 ? (
        <EmptyBlock>{t("dashboard.shareClientBans.empty")}</EmptyBlock>
      ) : (
        <div className="grid gap-2">
          {items.map((ban) => (
            <div
              key={ban.id}
              className="flex flex-col gap-3 rounded-xl border border-slate-200 bg-slate-50 px-3 py-3 sm:flex-row sm:items-center sm:justify-between"
            >
              <div className="flex min-w-0 items-start gap-2.5">
                <ShieldBan className="mt-0.5 h-4 w-4 shrink-0 text-amber-600" />
                <div className="min-w-0">
                  <div className="break-all font-mono text-sm font-semibold text-slate-900">
                    {ban.clientIp}
                  </div>
                  <div className="mt-1 text-xs leading-5 text-slate-500">
                    {banReasonLabel(ban.reasonCode, t)} · {t("dashboard.shareClientBans.failures", { count: ban.failureCount })}
                    <br />
                    {t("dashboard.shareClientBans.until", {
                      time: formatDateTime(ban.bannedUntil) || ban.bannedUntil,
                    })}
                  </div>
                </div>
              </div>
              <Button
                size="sm"
                variant="outline"
                onClick={() => void handleUnban(ban)}
                isDisabled={unbanningId !== null}
              >
                {unbanningId === ban.id ? (
                  <Loader2 className="h-4 w-4 animate-spin" />
                ) : (
                  <Unlock className="h-4 w-4" />
                )}
                {t("dashboard.shareClientBans.unban")}
              </Button>
            </div>
          ))}
          {nextCursor ? (
            <Button
              size="sm"
              variant="ghost"
              onClick={() => void load(nextCursor)}
              isDisabled={loadingMore}
            >
              {loadingMore ? <Loader2 className="h-4 w-4 animate-spin" /> : null}
              {t("dashboard.shareClientBans.loadMore")}
            </Button>
          ) : null}
        </div>
      )}
    </ShareEditSection>
  );
}
