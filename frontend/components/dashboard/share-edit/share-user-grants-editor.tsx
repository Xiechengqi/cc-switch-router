"use client";

import { Button, Checkbox, Input, ListBox, Modal, Select, Tooltip } from "@heroui/react";
import { Info, Pencil, Plus, Trash2 } from "lucide-react";
import * as React from "react";

import { ShareUserLimitsTable } from "@/components/dashboard/drawer-panels";
import type { TFn } from "@/components/dashboard/share-dashboard-utils";
import { getShareUserLimitStatus } from "@/lib/api";
import type {
  ShareTokenPeriod,
  ShareUserGrant,
  ShareUserGrantMap,
  ShareUserLimitStatusRow,
  ShareUserPolicy,
  ShareUserUsageEditMap,
} from "@/lib/types";
import {
  isRevokedRouterShareMarketGrant,
  ordinaryShareUserGrant,
  routerShareMarketManagedEmails,
} from "@/lib/share-settings";
import {
  formatTokenMillions,
  millionsInputToTokens,
  tokensToMillionsInput,
} from "@/lib/token-units";
import { applyShareUserPolicyBatch } from "./share-user-policy-batch";

type GrantDraft = {
  email: string;
  parallelLimit: string;
  tokenLimit: string;
  tokenPeriod: ShareTokenPeriod;
  tokenPeriodAnchor: string;
  expiresAt: string;
  consumedTokens: string;
  usageAction: "unchanged" | "set" | "clear";
};

type BatchGrantDraft = Omit<GrantDraft, "email"> & {
  applyParallelLimit: boolean;
  applyTokenLimit: boolean;
  applyConsumedTokens: boolean;
  applyExpiresAt: boolean;
};

const ANCHORED_PERIODS: ReadonlySet<ShareTokenPeriod> = new Set(["sevenDays", "thirtyDays"]);

function toLocalDateTime(value?: number) {
  if (!value) return "";
  const date = new Date(value);
  if (!Number.isFinite(date.getTime())) return "";
  const pad = (part: number) => String(part).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

function toUtcDateTime(value?: number) {
  const date = new Date(value ?? Math.floor(Date.now() / 60_000) * 60_000);
  if (!Number.isFinite(date.getTime())) return "";
  const pad = (part: number) => String(part).padStart(2, "0");
  return `${date.getUTCFullYear()}-${pad(date.getUTCMonth() + 1)}-${pad(date.getUTCDate())}T${pad(date.getUTCHours())}:${pad(date.getUTCMinutes())}`;
}

function parseUtcDateTime(value: string) {
  return value ? new Date(`${value}:00Z`).getTime() : undefined;
}

function makeDraft(email: string, policy: ShareUserPolicy): GrantDraft {
  return {
    email,
    parallelLimit: policy.parallelLimit == null ? "" : String(policy.parallelLimit),
    tokenLimit: policy.tokenLimit == null ? "" : tokensToMillionsInput(policy.tokenLimit),
    tokenPeriod: policy.tokenPeriod || "lifetime",
    tokenPeriodAnchor: toUtcDateTime(policy.tokenPeriodAnchorAtMs),
    expiresAt: toLocalDateTime(policy.expiresAt),
    consumedTokens: "",
    usageAction: "unchanged",
  };
}

function currentGrantTokens(grant: ShareUserGrant): number {
  if (grant.usageQuota) return grant.usageQuota.effectiveTokensUsed;
  const usage = grant.usage as
    | {
        day?: { tokensUsed?: number };
        week?: { tokensUsed?: number };
        calendarMonth?: { tokensUsed?: number };
        anchored?: { period?: ShareTokenPeriod; tokensUsed?: number };
        lifetime?: { tokensUsed?: number };
      }
    | undefined;
  if (!usage) return 0;
  switch (grant.policy.tokenPeriod) {
    case "day":
      return usage.day?.tokensUsed ?? 0;
    case "week":
      return usage.week?.tokensUsed ?? 0;
    case "calendarMonth":
      return usage.calendarMonth?.tokensUsed ?? 0;
    case "sevenDays":
    case "thirtyDays":
      return usage.anchored?.period === grant.policy.tokenPeriod
        ? usage.anchored.tokensUsed ?? 0
        : 0;
    case "lifetime":
    default:
      return usage.lifetime?.tokensUsed ?? 0;
  }
}

function observedGrantTokens(grant: ShareUserGrant): number {
  return grant.usageQuota?.observedTokensUsed ?? currentGrantTokens(grant);
}

function usageEditForGrant(
  grant: ShareUserGrant,
  usageEdits: ShareUserUsageEditMap,
): Pick<GrantDraft, "consumedTokens" | "usageAction"> {
  const edit = usageEdits[grant.email.trim().toLowerCase()];
  if (edit?.action === "clear") {
    return { consumedTokens: "", usageAction: "clear" };
  }
  if (edit?.action === "set") {
    return {
      consumedTokens: edit.targetTokens == null ? "" : tokensToMillionsInput(edit.targetTokens),
      usageAction: "set",
    };
  }
  return {
    consumedTokens: tokensToMillionsInput(currentGrantTokens(grant)),
    usageAction: "unchanged",
  };
}

function displayedGrantTokens(grant: ShareUserGrant, usageEdits: ShareUserUsageEditMap): number {
  const edit = usageEdits[grant.email.trim().toLowerCase()];
  if (edit?.action === "set" && edit.targetTokens != null) return edit.targetTokens;
  if (edit?.action === "clear") return observedGrantTokens(grant);
  return currentGrantTokens(grant);
}

function grantHasUsageOverride(grant: ShareUserGrant, usageEdits: ShareUserUsageEditMap) {
  const edit = usageEdits[grant.email.trim().toLowerCase()];
  return edit?.action === "set" || edit?.action === "clear";
}

function grantToLimitStatusRow(
  grant: ShareUserGrant,
  usageEdits: ShareUserUsageEditMap,
  liveRow?: ShareUserLimitStatusRow,
): ShareUserLimitStatusRow {
  const period = grant.policy.tokenPeriod || "lifetime";
  const livePeriodMatches = (liveRow?.tokenPeriod || "lifetime") === period;
  return {
    email: grant.email,
    role: grant.role,
    manager: grant.manager,
    parallelLimit: grant.policy.parallelLimit,
    tokenLimit: grant.policy.tokenLimit,
    tokenPeriod: period,
    tokenPeriodAnchorAtMs: grant.policy.tokenPeriodAnchorAtMs,
    expiresAt: grant.policy.expiresAt,
    tokensUsed: grantHasUsageOverride(grant, usageEdits) || !liveRow
      ? displayedGrantTokens(grant, usageEdits)
      : liveRow.tokensUsed || 0,
    windowStartsAt: livePeriodMatches ? liveRow?.windowStartsAt : undefined,
    resetsAt: livePeriodMatches ? liveRow?.resetsAt : undefined,
  };
}

function makeBatchDraft(grant: ShareUserGrant, usageEdits: ShareUserUsageEditMap): BatchGrantDraft {
  const draft = makeDraft(grant.email, grant.policy);
  const usage = usageEditForGrant(grant, usageEdits);
  return {
    parallelLimit: draft.parallelLimit,
    tokenLimit: draft.tokenLimit,
    tokenPeriod: draft.tokenPeriod,
    tokenPeriodAnchor: draft.tokenPeriodAnchor,
    expiresAt: draft.expiresAt,
    consumedTokens: usage.consumedTokens,
    usageAction: usage.usageAction === "clear" ? "clear" : "set",
    applyParallelLimit: true,
    applyTokenLimit: true,
    applyConsumedTokens: true,
    applyExpiresAt: true,
  };
}

function validEmail(value: string) {
  return /^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(value);
}

export function ShareUserGrantsEditor({
  value,
  ownerEmail,
  defaultPolicy,
  supportedPeriods,
  t,
  disabled,
  shareId,
  usageEdits = {},
  onUsageEditsChange,
  onChange,
}: {
  value: ShareUserGrantMap;
  ownerEmail: string;
  defaultPolicy: ShareUserPolicy;
  supportedPeriods?: ShareTokenPeriod[];
  t: TFn;
  disabled?: boolean;
  shareId?: string;
  usageEdits?: ShareUserUsageEditMap;
  onUsageEditsChange?: (value: ShareUserUsageEditMap) => void;
  onChange: (value: ShareUserGrantMap) => void;
}) {
  const normalizedOwner = ownerEmail.trim().toLowerCase();
  const [editingEmail, setEditingEmail] = React.useState<string | null>(null);
  const [grantDraft, setGrantDraft] = React.useState<GrantDraft | null>(null);
  const [selecting, setSelecting] = React.useState(false);
  const [selectedEmails, setSelectedEmails] = React.useState<Set<string>>(new Set());
  const [batchDraft, setBatchDraft] = React.useState<BatchGrantDraft | null>(null);
  const [error, setError] = React.useState("");
  const [liveRows, setLiveRows] = React.useState<ShareUserLimitStatusRow[] | null>(null);
  const supported = new Set<ShareTokenPeriod>(
    supportedPeriods?.length
      ? supportedPeriods
      : ["lifetime", "day", "week", "calendarMonth"],
  );
  const periods = ([
    { key: "lifetime", label: t("dashboard.userLimit.periodLifetime") },
    { key: "day", label: t("dashboard.userLimit.periodDay") },
    { key: "week", label: t("dashboard.userLimit.periodWeek") },
    { key: "sevenDays", label: t("dashboard.userLimit.periodSevenDays") },
    { key: "calendarMonth", label: t("dashboard.userLimit.periodMonth") },
    { key: "thirtyDays", label: t("dashboard.userLimit.periodThirtyDays") },
  ] satisfies Array<{ key: ShareTokenPeriod; label: string }>).filter((period) =>
    supported.has(period.key),
  );
  const periodLabel = Object.fromEntries(periods.map((period) => [period.key, period.label]));
  const shareMarketManagedEmails = routerShareMarketManagedEmails(value);
  const protectedEmails = shareMarketManagedEmails;
  const visibleEmails = new Set([
    normalizedOwner,
    ...Object.values(value)
      .filter((grant) => grant.active !== false)
      .map((grant) => grant.email),
  ]);
  const grants = Array.from(visibleEmails)
    .filter(Boolean)
    .map((email) => value[email] ?? ({
      email,
      role: email === normalizedOwner ? "owner" : "shareto",
      active: true,
      policy: { ...defaultPolicy },
    } satisfies ShareUserGrant))
    .filter((grant) => grant.active !== false)
    .sort((left, right) => {
      if (left.role === "owner") return -1;
      if (right.role === "owner") return 1;
      return left.email.localeCompare(right.email);
    });
  const selectableEmails = grants
    .filter((grant) => !protectedEmails.has(grant.email))
    .map((grant) => grant.email);
  const selectableEmailKey = selectableEmails.join("\0");
  const selectedEditableEmails = new Set(
    selectableEmails.filter((email) => selectedEmails.has(email)),
  );
  const allSelected = selectableEmails.length > 0 &&
    selectableEmails.every((email) => selectedEditableEmails.has(email));
  const someSelected = selectedEditableEmails.size > 0;

  React.useEffect(() => {
    const selectable = new Set(selectableEmailKey ? selectableEmailKey.split("\0") : []);
    setSelectedEmails((current) => {
      const next = new Set(Array.from(current).filter((email) => selectable.has(email)));
      if (next.size === current.size && Array.from(next).every((email) => current.has(email))) {
        return current;
      }
      return next;
    });
  }, [selectableEmailKey]);

  React.useEffect(() => {
    setLiveRows(null);
  }, [shareId]);

  React.useEffect(() => {
    if (!shareId) return;
    let cancelled = false;
    const load = async () => {
      try {
        const data = await getShareUserLimitStatus(shareId);
        if (cancelled) return;
        setLiveRows(data.rows || []);
      } catch {
        if (!cancelled) setLiveRows((current) => current ?? []);
      }
    };
    void load();
    const timer = window.setInterval(() => void load(), 15_000);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [shareId]);

  const liveRowByEmail = React.useMemo(() => {
    const map = new Map<string, ShareUserLimitStatusRow>();
    for (const row of liveRows || []) {
      map.set(row.email.trim().toLowerCase(), row);
    }
    return map;
  }, [liveRows]);

  const displayRows = grants.map((grant) =>
    grantToLimitStatusRow(
      grant,
      usageEdits,
      liveRowByEmail.get(grant.email.trim().toLowerCase()),
    ),
  );

  const grantByEmail = React.useMemo(() => {
    const map = new Map<string, ShareUserGrant>();
    for (const grant of grants) map.set(grant.email, grant);
    return map;
  }, [grants]);

  const openAdd = () => {
    setEditingEmail(null);
    setError("");
    setGrantDraft(makeDraft("", defaultPolicy));
  };

  const openEdit = (grant: ShareUserGrant) => {
    if (shareMarketManagedEmails.has(grant.email)) return;
    setEditingEmail(grant.email);
    setError("");
    setGrantDraft({
      ...makeDraft(grant.email, grant.policy),
      ...usageEditForGrant(grant, usageEdits),
    });
  };

  const exitSelecting = () => {
    setSelecting(false);
    setSelectedEmails(new Set());
  };

  const openBatchEdit = () => {
    if (!selecting) {
      setSelecting(true);
      return;
    }
    const firstSelected = grants.find((grant) => selectedEditableEmails.has(grant.email));
    if (!firstSelected) return;
    setError("");
    setBatchDraft(makeBatchDraft(firstSelected, usageEdits));
  };

  const applyGrants = (userGrants: ShareUserGrantMap) => onChange(userGrants);

  const save = () => {
    if (!grantDraft) return;
    const email = grantDraft.email.trim().toLowerCase();
    const parallelLimit = grantDraft.parallelLimit.trim()
      ? Number(grantDraft.parallelLimit)
      : undefined;
    const hasTokenLimit = !!grantDraft.tokenLimit.trim();
    const tokenLimit = hasTokenLimit
      ? millionsInputToTokens(grantDraft.tokenLimit)
      : undefined;
    const expiresAt = grantDraft.expiresAt
      ? new Date(grantDraft.expiresAt).getTime()
      : undefined;
    const anchored = ANCHORED_PERIODS.has(grantDraft.tokenPeriod);
    const tokenPeriodAnchorAtMs = anchored
      ? parseUtcDateTime(grantDraft.tokenPeriodAnchor)
      : undefined;
    if (!validEmail(email)) {
      setError(t("dashboard.userLimit.invalidEmail"));
      return;
    }
    if (
      (editingEmail && shareMarketManagedEmails.has(editingEmail)) ||
      (!editingEmail && shareMarketManagedEmails.has(email))
    ) {
      setError(t("dashboard.userLimit.marketManagedEmail"));
      return;
    }
    if (!editingEmail && value[email]?.active !== false && value[email]) {
      setError(t("dashboard.userLimit.duplicateEmail"));
      return;
    }
    if (
      (parallelLimit != null && (!Number.isInteger(parallelLimit) || parallelLimit < 1)) ||
      (hasTokenLimit && (tokenLimit == null || tokenLimit < 1)) ||
      (expiresAt != null && !Number.isFinite(expiresAt)) ||
      (anchored && (
        tokenPeriodAnchorAtMs == null ||
        !Number.isFinite(tokenPeriodAnchorAtMs) ||
        tokenPeriodAnchorAtMs > Math.floor(Date.now() / 60_000) * 60_000
      ))
    ) {
      setError(t("dashboard.userLimit.invalidPolicy"));
      return;
    }
    const previous = value[editingEmail || email];
    const reuseMarketTombstone = isRevokedRouterShareMarketGrant(previous);
    const consumedTokens = grantDraft.consumedTokens.trim()
      ? millionsInputToTokens(grantDraft.consumedTokens)
      : undefined;
    const observedTokens = previous && !reuseMarketTombstone ? observedGrantTokens(previous) : 0;
    const usageInvalid =
      grantDraft.usageAction === "set" &&
      (consumedTokens == null ||
        consumedTokens < 0 ||
        consumedTokens < observedTokens);
    if (usageInvalid) {
      setError(
        consumedTokens != null && consumedTokens < observedTokens
          ? t("dashboard.userLimit.consumedBelowObserved", {
              observed: formatTokenMillions(observedTokens),
            })
          : t("dashboard.userLimit.invalidUsage"),
      );
      return;
    }
    const next: ShareUserGrant = ordinaryShareUserGrant(email, normalizedOwner, previous, {
      parallelLimit,
      tokenLimit: tokenLimit ?? undefined,
      tokenPeriod: grantDraft.tokenPeriod,
      tokenPeriodAnchorAtMs,
      expiresAt,
    });
    if (grantDraft.usageAction === "set" && consumedTokens != null) {
      const previousQuota = previous?.usageQuota;
      next.usageQuota = {
        period: grantDraft.tokenPeriod,
        anchorAtMs: tokenPeriodAnchorAtMs,
        windowStartsAtMs: previousQuota?.windowStartsAtMs,
        windowEndsAtMs: previousQuota?.windowEndsAtMs,
        effectiveTokensUsed: consumedTokens,
        observedTokensUsed: observedTokens,
        manualOffsetTokens: consumedTokens - observedTokens,
        observedRequestsCount: previousQuota?.observedRequestsCount ?? 0,
        rebaseApplies: true,
      };
    } else if (grantDraft.usageAction === "clear") {
      const previousQuota = previous?.usageQuota;
      next.usageRebase = undefined;
      next.usageQuota = previousQuota
        ? {
            ...previousQuota,
            effectiveTokensUsed: observedTokens,
            observedTokensUsed: observedTokens,
            manualOffsetTokens: 0,
            rebaseApplies: false,
          }
        : undefined;
    }
    const userGrants = { ...value };
    if (editingEmail && editingEmail !== email) delete userGrants[editingEmail];
    userGrants[email] = next;
    applyGrants(userGrants);
    if (onUsageEditsChange) {
      const nextEdits: ShareUserUsageEditMap = { ...usageEdits };
      if (editingEmail && editingEmail !== email) delete nextEdits[editingEmail];
      if (grantDraft.usageAction === "set" && consumedTokens != null) {
        nextEdits[email] = {
          action: "set",
          targetTokens: consumedTokens,
          expectedGrantRevision: previous?.revision,
          period: grantDraft.tokenPeriod,
          anchorAtMs: tokenPeriodAnchorAtMs,
          source: usageEdits[editingEmail || email]?.source ?? "manual",
        };
      } else if (grantDraft.usageAction === "clear" && previous?.usageRebase) {
        nextEdits[email] = {
          action: "clear",
          expectedGrantRevision: previous.revision,
          period: grantDraft.tokenPeriod,
          anchorAtMs: tokenPeriodAnchorAtMs,
        };
      } else if (grantDraft.usageAction === "unchanged") {
        delete nextEdits[email];
      }
      onUsageEditsChange(nextEdits);
    }
    setGrantDraft(null);
  };

  const saveBatch = () => {
    if (!batchDraft || selectedEditableEmails.size === 0) return;
    const parallelLimit = batchDraft.parallelLimit.trim()
      ? Number(batchDraft.parallelLimit)
      : undefined;
    const hasTokenLimit = !!batchDraft.tokenLimit.trim();
    const tokenLimit = hasTokenLimit
      ? millionsInputToTokens(batchDraft.tokenLimit)
      : undefined;
    const expiresAt = batchDraft.expiresAt
      ? new Date(batchDraft.expiresAt).getTime()
      : undefined;
    const anchored = ANCHORED_PERIODS.has(batchDraft.tokenPeriod);
    const tokenPeriodAnchorAtMs = anchored
      ? parseUtcDateTime(batchDraft.tokenPeriodAnchor)
      : undefined;
    const consumedTokens = batchDraft.consumedTokens.trim()
      ? millionsInputToTokens(batchDraft.consumedTokens)
      : undefined;
    if (
      !batchDraft.applyParallelLimit &&
      !batchDraft.applyTokenLimit &&
      !batchDraft.applyConsumedTokens &&
      !batchDraft.applyExpiresAt
    ) {
      return;
    }
    const selectedGrants = grants.filter((grant) => selectedEditableEmails.has(grant.email));
    const usageFloor = selectedGrants.reduce(
      (highest, grant) => Math.max(highest, observedGrantTokens(grant)),
      0,
    );
    const usageInvalid =
      batchDraft.applyConsumedTokens &&
      batchDraft.usageAction === "set" &&
      (consumedTokens == null ||
        consumedTokens < 0 ||
        consumedTokens < usageFloor);
    if (
      (batchDraft.applyParallelLimit && parallelLimit != null &&
        (!Number.isInteger(parallelLimit) || parallelLimit < 1)) ||
      (batchDraft.applyTokenLimit && hasTokenLimit &&
        (tokenLimit == null || tokenLimit < 1)) ||
      (batchDraft.applyExpiresAt && expiresAt != null && !Number.isFinite(expiresAt)) ||
      (batchDraft.applyTokenLimit && anchored && (
        tokenPeriodAnchorAtMs == null ||
        !Number.isFinite(tokenPeriodAnchorAtMs) ||
        tokenPeriodAnchorAtMs > Math.floor(Date.now() / 60_000) * 60_000
      )) ||
      usageInvalid
    ) {
      setError(
        usageInvalid
          ? consumedTokens != null && consumedTokens < usageFloor
            ? t("dashboard.userLimit.consumedBelowObserved", {
                observed: formatTokenMillions(usageFloor),
              })
            : t("dashboard.userLimit.invalidUsage")
          : t("dashboard.userLimit.invalidPolicy"),
      );
      return;
    }

    const batchSourceGrants = { ...value };
    for (const grant of grants) {
      batchSourceGrants[grant.email] ??= grant;
    }
    const nextGrants = applyShareUserPolicyBatch(batchSourceGrants, selectedEditableEmails, {
      ...(batchDraft.applyParallelLimit
        ? { parallelLimit: { value: parallelLimit } }
        : {}),
      ...(batchDraft.applyTokenLimit
        ? {
            tokenLimit: {
              value: tokenLimit ?? undefined,
              period: batchDraft.tokenPeriod,
              periodAnchorAtMs: tokenPeriodAnchorAtMs,
            },
          }
        : {}),
      ...(batchDraft.applyExpiresAt
        ? { expiresAt: { value: expiresAt } }
        : {}),
    });
    if (batchDraft.applyConsumedTokens) {
      for (const grant of selectedGrants) {
        const current = nextGrants[grant.email] ?? grant;
        const observed = observedGrantTokens(grant);
        const period = batchDraft.applyTokenLimit
          ? batchDraft.tokenPeriod
          : current.policy.tokenPeriod;
        const anchorAtMs = batchDraft.applyTokenLimit
          ? tokenPeriodAnchorAtMs
          : current.policy.tokenPeriodAnchorAtMs;
        if (batchDraft.usageAction === "set" && consumedTokens != null) {
          const previousQuota = current.usageQuota;
          nextGrants[grant.email] = {
            ...current,
            usageQuota: {
              period,
              anchorAtMs,
              windowStartsAtMs: previousQuota?.windowStartsAtMs,
              windowEndsAtMs: previousQuota?.windowEndsAtMs,
              effectiveTokensUsed: consumedTokens,
              observedTokensUsed: observed,
              manualOffsetTokens: consumedTokens - observed,
              observedRequestsCount: previousQuota?.observedRequestsCount ?? 0,
              rebaseApplies: true,
            },
          };
        } else if (batchDraft.usageAction === "clear") {
          const previousQuota = current.usageQuota;
          nextGrants[grant.email] = {
            ...current,
            usageRebase: undefined,
            usageQuota: previousQuota
              ? {
                  ...previousQuota,
                  effectiveTokensUsed: observed,
                  observedTokensUsed: observed,
                  manualOffsetTokens: 0,
                  rebaseApplies: false,
                }
              : undefined,
          };
        }
      }
    }
    applyGrants(nextGrants);
    if (onUsageEditsChange && (batchDraft.applyTokenLimit || batchDraft.applyConsumedTokens)) {
      const nextEdits: ShareUserUsageEditMap = { ...usageEdits };
      for (const grant of selectedGrants) {
        const email = grant.email.trim().toLowerCase();
        if (batchDraft.applyConsumedTokens && batchDraft.usageAction === "set" && consumedTokens != null) {
          nextEdits[email] = {
            action: "set",
            targetTokens: consumedTokens,
            expectedGrantRevision: grant.revision,
            period: batchDraft.applyTokenLimit ? batchDraft.tokenPeriod : grant.policy.tokenPeriod,
            anchorAtMs: batchDraft.applyTokenLimit
              ? tokenPeriodAnchorAtMs
              : grant.policy.tokenPeriodAnchorAtMs,
            source: usageEdits[email]?.source ?? "manual",
          };
        } else if (batchDraft.applyConsumedTokens && batchDraft.usageAction === "clear" && grant.usageRebase) {
          nextEdits[email] = {
            action: "clear",
            expectedGrantRevision: grant.revision,
            period: batchDraft.applyTokenLimit ? batchDraft.tokenPeriod : grant.policy.tokenPeriod,
            anchorAtMs: batchDraft.applyTokenLimit
              ? tokenPeriodAnchorAtMs
              : grant.policy.tokenPeriodAnchorAtMs,
          };
        } else {
          delete nextEdits[email];
        }
      }
      onUsageEditsChange(nextEdits);
    }
    setSelectedEmails(new Set());
    setBatchDraft(null);
    setError("");
    setSelecting(false);
  };

  const removeGrant = (grant: ShareUserGrant) => {
    const userGrants = { ...value };
    delete userGrants[grant.email];
    applyGrants(userGrants);
    if (onUsageEditsChange) {
      const nextEdits = { ...usageEdits };
      delete nextEdits[grant.email];
      onUsageEditsChange(nextEdits);
    }
  };

  return (
    <div className="grid gap-3">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div className="flex items-center gap-1">
          <div className="mono-label text-muted-foreground">{t("dashboard.userLimit.title")}</div>
          <Tooltip>
            <Tooltip.Trigger>
              <Button
                isIconOnly
                size="sm"
                variant="ghost"
                className="h-6 w-6 min-w-6 text-muted-foreground"
                aria-label={t("dashboard.userLimit.parallelScopeHint")}
              >
                <Info className="h-3.5 w-3.5" />
              </Button>
            </Tooltip.Trigger>
            <Tooltip.Content>{t("dashboard.userLimit.parallelScopeHint")}</Tooltip.Content>
          </Tooltip>
        </div>
        <div className="flex flex-wrap items-center justify-end gap-2">
          {selecting ? (
            <Button size="sm" variant="ghost" onClick={exitSelecting}>
              {t("common.cancel")}
            </Button>
          ) : null}
          <Button
            size="sm"
            variant="outline"
            isDisabled={
              disabled ||
              selectableEmails.length === 0 ||
              (selecting && selectedEditableEmails.size === 0)
            }
            onClick={openBatchEdit}
          >
            <Pencil className="h-4 w-4" />
            {selecting
              ? t("dashboard.userLimit.batchEditSelected", { count: selectedEditableEmails.size })
              : t("dashboard.userLimit.batchEdit")}
          </Button>
          <Button size="sm" variant="outline" isDisabled={disabled} onClick={openAdd}>
            <Plus className="h-4 w-4" />
            {t("dashboard.userLimit.add")}
          </Button>
        </div>
      </div>

      {grants.length ? (
        <ShareUserLimitsTable
          rows={displayRows}
          grants={grants}
          t={t}
          leading={selecting ? {
            header: (
              <Checkbox
                isSelected={allSelected}
                isIndeterminate={someSelected && !allSelected}
                isDisabled={!selectableEmails.length}
                onChange={(checked) =>
                  setSelectedEmails(new Set(checked ? selectableEmails : []))
                }
                aria-label={t("dashboard.userLimit.selectAll")}
                className="shrink-0"
              >
                <Checkbox.Control className="border border-slate-300 bg-white shadow-none">
                  <Checkbox.Indicator />
                </Checkbox.Control>
              </Checkbox>
            ),
            cell: (row) => {
              const grant = grantByEmail.get(row.email);
              if (!grant) return null;
              return (
                <Checkbox
                  isSelected={selectedEmails.has(grant.email)}
                  isDisabled={protectedEmails.has(grant.email)}
                  onChange={(checked) => {
                    setSelectedEmails((current) => {
                      const next = new Set(current);
                      if (checked) next.add(grant.email);
                      else next.delete(grant.email);
                      return next;
                    });
                  }}
                  aria-label={t("dashboard.userLimit.selectUser", { email: grant.email })}
                  className="shrink-0"
                >
                  <Checkbox.Control className="border border-slate-300 bg-white shadow-none">
                    <Checkbox.Indicator />
                  </Checkbox.Control>
                </Checkbox>
              );
            },
          } : undefined}
          trailing={{
            cell: (row) => {
              const grant = grantByEmail.get(row.email);
              if (!grant) return null;
              const marketManaged = shareMarketManagedEmails.has(grant.email);
              return (
                <div className="flex items-center justify-center gap-0.5">
                  {!marketManaged ? (
                    <Button
                      isIconOnly
                      size="sm"
                      variant="ghost"
                      className="h-6 w-6 min-w-6"
                      aria-label={t("common.edit")}
                      isDisabled={disabled}
                      onClick={() => openEdit(grant)}
                    >
                      <Pencil className="h-3.5 w-3.5" />
                    </Button>
                  ) : null}
                  {grant.role !== "owner" && !protectedEmails.has(grant.email) ? (
                    <Button
                      isIconOnly
                      size="sm"
                      variant="ghost"
                      className="h-6 w-6 min-w-6"
                      aria-label={t("common.delete")}
                      isDisabled={disabled}
                      onClick={() => removeGrant(grant)}
                    >
                      <Trash2 className="h-3.5 w-3.5 text-red-600" />
                    </Button>
                  ) : null}
                </div>
              );
            },
          }}
        />
      ) : null}

      <Modal.Backdrop
        isOpen={!!grantDraft}
        onOpenChange={(open) => !open && setGrantDraft(null)}
        className="z-[70]"
      >
          <Modal.Container placement="center" className="z-[70]">
            <Modal.Dialog className="light w-[min(620px,calc(100vw-2rem))] max-w-none !bg-white !text-slate-900">
              <Modal.CloseTrigger />
              <Modal.Header>
                <Modal.Heading>{editingEmail ? t("dashboard.userLimit.edit") : t("dashboard.userLimit.add")}</Modal.Heading>
              </Modal.Header>
              <Modal.Body className="grid gap-4 !text-slate-900 sm:grid-cols-2">
                <div className="grid gap-1.5 sm:col-span-2">
                  <span className="mono-label text-muted-foreground">Email</span>
                  <Input type="email" value={grantDraft?.email || ""} disabled={!!editingEmail} onChange={(event) => grantDraft && setGrantDraft({ ...grantDraft, email: event.target.value })} />
                </div>
                <div className="grid gap-1.5">
                  <span className="mono-label text-muted-foreground">{t("dashboard.field.parallelLimit")}</span>
                  <Input type="number" min={1} placeholder={t("common.unlimited")} value={grantDraft?.parallelLimit || ""} onChange={(event) => grantDraft && setGrantDraft({ ...grantDraft, parallelLimit: event.target.value })} />
                </div>
                <div className="grid gap-1.5">
                  <span className="mono-label text-muted-foreground">{t("dashboard.field.tokenLimit")}</span>
                  <Input type="text" inputMode="decimal" placeholder={t("common.unlimited")} value={grantDraft?.tokenLimit || ""} onChange={(event) => grantDraft && setGrantDraft({ ...grantDraft, tokenLimit: event.target.value })} />
                </div>
                <div className="grid gap-1.5 sm:col-span-2">
                  <span className="mono-label text-muted-foreground">{t("dashboard.userLimit.consumedTokens")}</span>
                  <div className="flex items-center gap-2">
                    <Input
                      type="text"
                      inputMode="decimal"
                      placeholder="0"
                      value={grantDraft?.consumedTokens || ""}
                      onChange={(event) => grantDraft && setGrantDraft({
                        ...grantDraft,
                        consumedTokens: event.target.value,
                        usageAction: "set",
                      })}
                    />
                    <Button
                      variant="outline"
                      size="sm"
                      isDisabled={
                        !editingEmail ||
                        (!value[editingEmail]?.usageRebase && usageEdits[editingEmail]?.action !== "set")
                      }
                      onClick={() => grantDraft && setGrantDraft({
                        ...grantDraft,
                        consumedTokens: "",
                        usageAction: "clear",
                      })}
                    >
                      {t("dashboard.userLimit.clearRebase")}
                    </Button>
                  </div>
                  {editingEmail && value[editingEmail] ? (
                    <p className="text-xs text-muted-foreground">
                      {t("dashboard.userLimit.consumedHint", {
                        effective: formatTokenMillions(currentGrantTokens(value[editingEmail])),
                        observed: formatTokenMillions(observedGrantTokens(value[editingEmail])),
                      })}
                    </p>
                  ) : (
                    <p className="text-xs text-muted-foreground">{t("dashboard.userLimit.newConsumedHint")}</p>
                  )}
                </div>
                <div className="grid gap-1.5">
                  <span className="mono-label text-muted-foreground">{t("dashboard.userLimit.period")}</span>
                  <Select selectedKey={grantDraft?.tokenPeriod || "lifetime"} onSelectionChange={(key) => {
                    if (!grantDraft) return;
                    const tokenPeriod = String(key || "lifetime") as ShareTokenPeriod;
                    setGrantDraft({
                      ...grantDraft,
                      tokenPeriod,
                      tokenPeriodAnchor: ANCHORED_PERIODS.has(tokenPeriod)
                        ? (grantDraft.tokenPeriodAnchor || toUtcDateTime())
                        : "",
                    });
                  }}>
                    <Select.Trigger><Select.Value>{periodLabel[grantDraft?.tokenPeriod || "lifetime"]}</Select.Value><Select.Indicator /></Select.Trigger>
                    <Select.Popover className="share-edit-popover light z-[80] !bg-white !text-slate-900">
                      <ListBox>{periods.map((period) => <ListBox.Item key={period.key} id={period.key}>{period.label}</ListBox.Item>)}</ListBox>
                    </Select.Popover>
                  </Select>
                </div>
                {grantDraft && ANCHORED_PERIODS.has(grantDraft.tokenPeriod) ? (
                  <div className="grid gap-1.5 sm:col-span-2">
                    <span className="mono-label text-muted-foreground">{t("dashboard.userLimit.anchor")}</span>
                    <Input
                      type="datetime-local"
                      step={60}
                      value={grantDraft.tokenPeriodAnchor}
                      onChange={(event) => setGrantDraft({ ...grantDraft, tokenPeriodAnchor: event.target.value })}
                    />
                    <p className="text-xs text-muted-foreground">{t("dashboard.userLimit.anchorHint")}</p>
                  </div>
                ) : null}
                <div className="grid gap-1.5">
                  <span className="mono-label text-muted-foreground">{t("dashboard.field.expiresAt")}</span>
                  <Input type="datetime-local" value={grantDraft?.expiresAt || ""} onChange={(event) => grantDraft && setGrantDraft({ ...grantDraft, expiresAt: event.target.value })} />
                </div>
                {error ? <p className="text-sm text-red-600 sm:col-span-2">{error}</p> : null}
              </Modal.Body>
              <Modal.Footer>
                <Button variant="outline" onClick={() => setGrantDraft(null)}>{t("common.cancel")}</Button>
                <Button variant="primary" onClick={save}>{t("common.save")}</Button>
              </Modal.Footer>
            </Modal.Dialog>
          </Modal.Container>
      </Modal.Backdrop>

      <Modal.Backdrop
        isOpen={!!batchDraft}
        onOpenChange={(open) => {
          if (!open) {
            setBatchDraft(null);
            setError("");
          }
        }}
        className="z-[70]"
      >
        <Modal.Container placement="center" className="z-[70]">
          <Modal.Dialog className="light w-[min(620px,calc(100vw-2rem))] max-w-none !bg-white !text-slate-900">
            <Modal.CloseTrigger />
            <Modal.Header>
              <Modal.Heading>{t("dashboard.userLimit.batchTitle")}</Modal.Heading>
            </Modal.Header>
            <Modal.Body className="grid gap-4 !text-slate-900 sm:grid-cols-2">
              <p className="text-sm text-muted-foreground sm:col-span-2">
                {t("dashboard.userLimit.batchHint", { count: selectedEditableEmails.size })}
              </p>
              <div className="grid gap-2">
                <Checkbox
                  isSelected={batchDraft?.applyParallelLimit || false}
                  onChange={(checked) => batchDraft && setBatchDraft({
                    ...batchDraft,
                    applyParallelLimit: checked,
                  })}
                >
                  <Checkbox.Control className="border border-slate-300 bg-white shadow-none">
                    <Checkbox.Indicator />
                  </Checkbox.Control>
                  <Checkbox.Content>
                    <span className="mono-label text-muted-foreground">
                      {t("dashboard.field.parallelLimit")}
                    </span>
                  </Checkbox.Content>
                </Checkbox>
                <Input
                  type="number"
                  min={1}
                  disabled={!batchDraft?.applyParallelLimit}
                  placeholder={t("common.unlimited")}
                  value={batchDraft?.parallelLimit || ""}
                  onChange={(event) => batchDraft && setBatchDraft({
                    ...batchDraft,
                    parallelLimit: event.target.value,
                  })}
                />
              </div>
              <div className="grid gap-2">
                <Checkbox
                  isSelected={batchDraft?.applyTokenLimit || false}
                  onChange={(checked) => batchDraft && setBatchDraft({
                    ...batchDraft,
                    applyTokenLimit: checked,
                  })}
                >
                  <Checkbox.Control className="border border-slate-300 bg-white shadow-none">
                    <Checkbox.Indicator />
                  </Checkbox.Control>
                  <Checkbox.Content>
                    <span className="mono-label text-muted-foreground">
                      {t("dashboard.field.tokenLimit")}
                    </span>
                  </Checkbox.Content>
                </Checkbox>
                <Input
                  type="text"
                  inputMode="decimal"
                  disabled={!batchDraft?.applyTokenLimit}
                  placeholder={t("common.unlimited")}
                  value={batchDraft?.tokenLimit || ""}
                  onChange={(event) => batchDraft && setBatchDraft({
                    ...batchDraft,
                    tokenLimit: event.target.value,
                  })}
                />
              </div>
              <div className="grid gap-1.5">
                <span className="mono-label text-muted-foreground">
                  {t("dashboard.userLimit.period")}
                </span>
                <Select
                  selectedKey={batchDraft?.tokenPeriod || "lifetime"}
                  onSelectionChange={(key) => {
                    if (!batchDraft) return;
                    const tokenPeriod = String(key || "lifetime") as ShareTokenPeriod;
                    setBatchDraft({
                      ...batchDraft,
                      applyTokenLimit: true,
                      tokenPeriod,
                      tokenPeriodAnchor: ANCHORED_PERIODS.has(tokenPeriod)
                        ? (batchDraft.tokenPeriodAnchor || toUtcDateTime())
                        : "",
                    });
                  }}
                >
                  <Select.Trigger>
                    <Select.Value>{periodLabel[batchDraft?.tokenPeriod || "lifetime"]}</Select.Value>
                    <Select.Indicator />
                  </Select.Trigger>
                  <Select.Popover className="share-edit-popover light z-[80] !bg-white !text-slate-900">
                    <ListBox>
                      {periods.map((period) => (
                        <ListBox.Item key={period.key} id={period.key}>{period.label}</ListBox.Item>
                      ))}
                    </ListBox>
                  </Select.Popover>
                </Select>
              </div>
              <div className="grid gap-2 sm:col-span-2">
                <Checkbox
                  isSelected={batchDraft?.applyConsumedTokens || false}
                  onChange={(checked) => batchDraft && setBatchDraft({
                    ...batchDraft,
                    applyConsumedTokens: checked,
                  })}
                >
                  <Checkbox.Control className="border border-slate-300 bg-white shadow-none">
                    <Checkbox.Indicator />
                  </Checkbox.Control>
                  <Checkbox.Content>
                    <span className="mono-label text-muted-foreground">
                      {t("dashboard.userLimit.consumedTokens")}
                    </span>
                  </Checkbox.Content>
                </Checkbox>
                <div className="flex items-center gap-2">
                  <Input
                    type="text"
                    inputMode="decimal"
                    disabled={!batchDraft?.applyConsumedTokens}
                    placeholder="0"
                    value={batchDraft?.consumedTokens || ""}
                    onChange={(event) => batchDraft && setBatchDraft({
                      ...batchDraft,
                      consumedTokens: event.target.value,
                      usageAction: "set",
                    })}
                  />
                  <Button
                    variant="outline"
                    size="sm"
                    isDisabled={!batchDraft?.applyConsumedTokens}
                    onClick={() => batchDraft && setBatchDraft({
                      ...batchDraft,
                      consumedTokens: "",
                      usageAction: "clear",
                    })}
                  >
                    {t("dashboard.userLimit.clearRebase")}
                  </Button>
                </div>
                <p className="text-xs text-muted-foreground">
                  {t("dashboard.userLimit.batchConsumedHint")}
                </p>
              </div>
              {batchDraft?.applyTokenLimit && ANCHORED_PERIODS.has(batchDraft.tokenPeriod) ? (
                <div className="grid gap-1.5 sm:col-span-2">
                  <span className="mono-label text-muted-foreground">
                    {t("dashboard.userLimit.anchor")}
                  </span>
                  <Input
                    type="datetime-local"
                    step={60}
                    value={batchDraft.tokenPeriodAnchor}
                    onChange={(event) => setBatchDraft({
                      ...batchDraft,
                      tokenPeriodAnchor: event.target.value,
                    })}
                  />
                  <p className="text-xs text-muted-foreground">
                    {t("dashboard.userLimit.anchorHint")}
                  </p>
                </div>
              ) : null}
              <div className="grid gap-2 sm:col-span-2">
                <Checkbox
                  isSelected={batchDraft?.applyExpiresAt || false}
                  onChange={(checked) => batchDraft && setBatchDraft({
                    ...batchDraft,
                    applyExpiresAt: checked,
                  })}
                >
                  <Checkbox.Control className="border border-slate-300 bg-white shadow-none">
                    <Checkbox.Indicator />
                  </Checkbox.Control>
                  <Checkbox.Content>
                    <span className="mono-label text-muted-foreground">
                      {t("dashboard.field.expiresAt")}
                    </span>
                  </Checkbox.Content>
                </Checkbox>
                <Input
                  type="datetime-local"
                  disabled={!batchDraft?.applyExpiresAt}
                  value={batchDraft?.expiresAt || ""}
                  onChange={(event) => batchDraft && setBatchDraft({
                    ...batchDraft,
                    expiresAt: event.target.value,
                  })}
                />
              </div>
              {error ? <p className="text-sm text-red-600 sm:col-span-2">{error}</p> : null}
            </Modal.Body>
            <Modal.Footer>
              <Button variant="outline" onClick={() => {
                setBatchDraft(null);
                setError("");
              }}>
                {t("common.cancel")}
              </Button>
              <Button
                variant="primary"
                isDisabled={!!batchDraft &&
                  !batchDraft.applyParallelLimit &&
                  !batchDraft.applyTokenLimit &&
                  !batchDraft.applyConsumedTokens &&
                  !batchDraft.applyExpiresAt}
                onClick={saveBatch}
              >
                {t("dashboard.userLimit.batchApply", { count: selectedEditableEmails.size })}
              </Button>
            </Modal.Footer>
          </Modal.Dialog>
        </Modal.Container>
      </Modal.Backdrop>
    </div>
  );
}
