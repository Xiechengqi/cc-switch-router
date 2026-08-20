"use client";

import * as React from "react";
import { EmptyBlock, ShareUserLimitsTable } from "@/components/dashboard/drawer-panels";
import {
  expiryTitle,
  isOfficialRuntime,
  isUnlimitedParallelLimit,
  isUnlimitedTokenLimit,
  runtimeModelSummary,
  type TFn,
} from "@/components/dashboard/share-dashboard-utils";
import {
  formatShareCeilingParallel,
  formatShareCeilingToken,
  ShareCeilingBar,
} from "./share-ceiling-bar";
import { getShareUserLimitStatus } from "@/lib/api";
import { ShareAppLogo } from "@/components/dashboard/share-app-logo";
import { shareProviderSupportedApps, resolveShareCoreApp, SHARE_APP_LABELS } from "@/lib/share-app";
import type {
  ShareAppRuntimes,
  ShareUpstreamProvider,
  ShareUserGrant,
  ShareUserLimitStatusRow,
  ShareView,
} from "@/lib/types";
import { formatDateTime } from "@/lib/utils";
import {
  ReadOnlyField,
  ShareEditSection,
} from "./share-edit-section";

function activeUserLimitGrants(share: ShareView): ShareUserGrant[] {
  return Object.values(share.userGrants || {})
    .filter((grant) => grant.active !== false)
    .sort((left, right) => {
      if (left.role === "owner") return -1;
      if (right.role === "owner") return 1;
      return left.email.localeCompare(right.email);
    });
}

function looksLikeEmail(value: string) {
  return /^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(value.trim());
}

function providerHint(runtime?: ShareUpstreamProvider) {
  if (!runtime) return "";
  if (isOfficialRuntime(runtime)) return "Official";
  if (runtime.providerName?.trim()) return "";
  const hint = String(runtime.apiUrl || runtime.kind || "").trim();
  if (!hint || looksLikeEmail(hint)) return "";
  return hint;
}

export function ShareEditReadView({
  share,
  t,
}: {
  share: ShareView;
  t: TFn;
}) {
  const boundApps = shareProviderSupportedApps(share);
  const shareApp = resolveShareCoreApp(share) ?? boundApps[0];

  const freeAccess = share.freeAccess;

  const tokenLimit = share.tokenLimit;
  const parallelLimit = share.parallelLimit;
  const tokenUnlimited = isUnlimitedTokenLimit(tokenLimit);
  const parallelUnlimited = isUnlimitedParallelLimit(parallelLimit);
  const limitGrants = React.useMemo(() => activeUserLimitGrants(share), [share]);
  const [limitRows, setLimitRows] = React.useState<ShareUserLimitStatusRow[] | null>(null);
  const [limitLoading, setLimitLoading] = React.useState(false);
  const [limitError, setLimitError] = React.useState("");
  const hasLimitRowsRef = React.useRef(false);

  React.useEffect(() => {
    hasLimitRowsRef.current = false;
    setLimitRows(null);
    setLimitError("");
    setLimitLoading(false);
  }, [share.shareId]);

  React.useEffect(() => {
    if (!shareApp) {
      setLimitRows(null);
      setLimitError("");
      setLimitLoading(false);
      hasLimitRowsRef.current = false;
      return;
    }
    let cancelled = false;
    const load = async () => {
      const silent = hasLimitRowsRef.current;
      if (!silent) setLimitLoading(true);
      try {
        const data = await getShareUserLimitStatus(share.shareId);
        if (cancelled) return;
        setLimitRows(data.rows || []);
        setLimitError("");
        hasLimitRowsRef.current = true;
      } catch (err) {
        if (cancelled) return;
        if (!silent) {
          setLimitRows(null);
          setLimitError(err instanceof Error ? err.message : String(err));
          hasLimitRowsRef.current = false;
        }
      } finally {
        if (!cancelled && !silent) setLimitLoading(false);
      }
    };
    void load();
    const timer = window.setInterval(() => void load(), 15_000);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [share.shareId, share.configRevision, shareApp]);

  const description = share.description?.trim() || "";

  return (
    <div className="grid gap-6">
      {description ? (
        <div className="rounded-lg border border-slate-200 bg-slate-50 px-3 py-2 text-sm text-slate-900">
          {description}
        </div>
      ) : null}

      <ShareEditSection title={t("dashboard.shareEdit.section.market")}>
        <div className="grid grid-cols-3 gap-2">
          {boundApps.map((app) => {
            const runtime = share.appRuntimes?.[app as keyof ShareAppRuntimes];
            const hint = providerHint(runtime);
            const models = runtimeModelSummary(runtime, t("dashboard.shareEdit.passthrough"));
            const enabled = !(share.support && share.support[app] === false);
            return (
              <div
                key={app}
                className={`min-w-0 rounded-xl px-3 py-2.5 ${
                  enabled ? "bg-emerald-50 text-slate-900" : "bg-slate-50 text-slate-500"
                }`}
              >
                <div className="flex min-w-0 items-start gap-2">
                  <ShareAppLogo app={app} size={16} className={enabled ? undefined : "opacity-60"} />
                  <div className="min-w-0 flex-1">
                    <div className="flex min-w-0 flex-col gap-0.5">
                      <span className={`text-sm font-medium ${enabled ? "text-slate-900" : "text-slate-500"}`}>
                        {SHARE_APP_LABELS[app]} API
                      </span>
                      {runtime?.providerName && !looksLikeEmail(runtime.providerName) ? (
                        <span className="truncate text-xs text-slate-500">{runtime.providerName}</span>
                      ) : null}
                    </div>
                    <div className={`mt-0.5 whitespace-normal break-all text-[11px] ${enabled ? "text-slate-500" : "text-slate-400"}`}>
                      {[hint, models].filter(Boolean).join(" · ") || "—"}
                    </div>
                  </div>
                </div>
              </div>
            );
          })}
        </div>
      </ShareEditSection>

      {shareApp ? (
        <>
          <ShareEditSection title={t("dashboard.shareEdit.section.access")}>
            {freeAccess ? (
              <ReadOnlyField
                label={t("dashboard.field.freeAccess")}
                value={t("dashboard.freeAccessEnabled")}
              />
            ) : null}
            <div className="grid gap-2">
              <div className="text-sm font-semibold text-slate-900">{t("dashboard.userLimit.title")}</div>
              {limitRows?.length || limitGrants.length ? (
                <ShareUserLimitsTable rows={limitRows || undefined} grants={limitGrants} t={t} />
              ) : limitLoading ? (
                <EmptyBlock>{t("dashboard.userLimit.loading")}</EmptyBlock>
              ) : limitError ? (
                <EmptyBlock>{limitError}</EmptyBlock>
              ) : (
                <EmptyBlock>{t("dashboard.userLimit.empty")}</EmptyBlock>
              )}
            </div>
            <ShareCeilingBar
              t={t}
              tokenDisplay={formatShareCeilingToken(tokenLimit, tokenUnlimited, t)}
              parallelDisplay={formatShareCeilingParallel(parallelLimit, parallelUnlimited, t)}
              expiryDisplay={
                expiryTitle(share.expiresAt) === "∞"
                  ? t("dashboard.userLimit.permanent")
                  : formatDateTime(share.expiresAt) || "—"
              }
            />
          </ShareEditSection>
        </>
      ) : (
        <EmptyBlock>{t("dashboard.shareEditNoAppType")}</EmptyBlock>
      )}
    </div>
  );
}
