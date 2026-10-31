"use client";

import { Button, Checkbox, Input, ListBox, Modal, Select, Tooltip } from "@heroui/react";
import { Info, Pencil, Plus, Trash2 } from "lucide-react";
import * as React from "react";

import { ShareUserLimitsTable } from "@/components/dashboard/drawer-panels";
import type { TFn } from "@/components/dashboard/share-dashboard-utils";
import { getShareUserLimitStatus, previewShareAccountReset } from "@/lib/api";
import { useShareUserUsageBreakdown } from "@/lib/use-share-user-usage-breakdown";
import { useValueQuotaReadiness } from "@/lib/use-value-quota-readiness";
import { useValueQuotaPreview } from "@/lib/use-value-quota-preview";
import { ValueQuotaReadiness } from "./value-quota-readiness";
import { QuotaLimitField, QuotaUnitSwitch, ValuePreviewLine } from "./value-quota-fields";
import type {
  ShareTokenPeriod,
  ShareUserGrant,
  ShareUserGrantMap,
  ShareUserLimitStatusRow,
  ShareUserPolicy,
  ShareUserUsageEditMap,
  AccountResetPreview,
} from "@/lib/types";
import {
  isRevokedRouterShareMarketGrant,
  isPermanentUserPolicyExpiry,
  ordinaryShareUserGrant,
  routerShareMarketManagedEmails,
  sharePolicyFingerprint,
  stageUserPolicy,
} from "@/lib/share-settings";
import {
  formatTokenMillions,
  millionsInputToTokens,
  tokensToMillionsInput,
} from "@/lib/token-units";
import { applyShareUserPolicyBatch, patchedUserPolicy, type ShareUserPolicyBatchPatch } from "./share-user-policy-batch";
import { ShareValueUsageAdjustment } from "./share-value-usage-adjustment";
import { publishValueQuotaStatuses, useShareValueQuotaSnapshot } from "@/lib/use-share-value-quota";
import {
  formatQuotaPicos, formatUsdLimit, microsToUsdInput, parseUsdLimit,
  usdAmountErrorKey, valuePreviewBlocker, valuePreviewNeedsAcknowledgement, valueQuotaView, type ValueQuotaPreview, type ValueQuotaStatus,
} from "@/lib/share-value-quota";

type GrantDraft = {
  quotaUnit: "tokens" | "usdEquivalent";
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
  if (!value || isPermanentUserPolicyExpiry(value)) return "";
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
    quotaUnit: policy.valueQuota ? "usdEquivalent" : "tokens",
    parallelLimit: policy.parallelLimit == null ? "" : String(policy.parallelLimit),
    tokenLimit: policy.valueQuota ? microsToUsdInput(policy.valueQuota.limitUsdMicros) : policy.tokenLimit == null ? "" : tokensToMillionsInput(policy.tokenLimit),
    tokenPeriod: policy.tokenPeriod || "lifetime",
    tokenPeriodAnchor: toUtcDateTime(policy.tokenPeriodAnchorAtMs),
    expiresAt: toLocalDateTime(policy.expiresAt),
    consumedTokens: "",
    usageAction: "unchanged",
  };
}

function currentGrantTokens(grant: ShareUserGrant): number {
  if (grant.usageQuota) return grant.usageQuota.effectiveTokensUsed;
  if (grant.policy.tokenPeriod === "accountReset") return 0; // rendered as pending below
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
    consumedTokens: grant.policy.tokenPeriod === "accountReset" && !grant.usageQuota
      ? "" : tokensToMillionsInput(currentGrantTokens(grant)),
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
  // A scheduled USD policy is a draft here; the live row still describes the
  // policy that is enforced until the next window.
  const enforced = grant.pendingValueQuota && liveRow ? liveRow : undefined;
  // Without that row, a draft holding the scheduled policy says nothing about
  // the enforced Token limit: "—", never an empty limit read as "∞".
  const limitUnknown = !liveRow && !!grant.pendingValueQuota?.policy
    && sharePolicyFingerprint(grant.policy) === sharePolicyFingerprint(grant.pendingValueQuota.policy);
  const period = enforced ? enforced.tokenPeriod || "lifetime" : grant.policy.tokenPeriod || "lifetime";
  const livePeriodMatches = (liveRow?.tokenPeriod || "lifetime") === period;
  if (limitUnknown) {
    return {
      email: grant.email, role: grant.role, manager: grant.manager, parallelLimit: grant.policy.parallelLimit,
      tokenLimit: undefined, tokenPeriod: period, tokenPeriodAnchorAtMs: grant.policy.tokenPeriodAnchorAtMs,
      expiresAt: grant.policy.expiresAt, tokensUsed: null, limitUnknown: true,
    };
  }
  return {
    email: grant.email,
    role: grant.role,
    manager: grant.manager,
    parallelLimit: grant.policy.parallelLimit,
    tokenLimit: enforced ? enforced.tokenLimit : grant.policy.tokenLimit,
    tokenPeriod: period,
    tokenPeriodAnchorAtMs: enforced ? enforced.tokenPeriodAnchorAtMs : grant.policy.tokenPeriodAnchorAtMs,
    expiresAt: grant.policy.expiresAt,
    tokensUsed: enforced ? enforced.tokensUsed : period === "accountReset" && !grant.usageQuota && !grantHasUsageOverride(grant, usageEdits)
      ? null : grantHasUsageOverride(grant, usageEdits) || !liveRow
      ? displayedGrantTokens(grant, usageEdits)
      : liveRow.tokensUsed,
    windowStartsAt: livePeriodMatches ? liveRow?.windowStartsAt : undefined,
    resetsAt: livePeriodMatches ? liveRow?.resetsAt : undefined,
  };
}

function makeBatchDraft(grant: ShareUserGrant): BatchGrantDraft {
  const draft = makeDraft(grant.email, grant.policy);
  return {
    quotaUnit: draft.quotaUnit,
    parallelLimit: draft.parallelLimit,
    tokenLimit: draft.tokenLimit,
    tokenPeriod: draft.tokenPeriod,
    tokenPeriodAnchor: draft.tokenPeriodAnchor,
    expiresAt: draft.expiresAt,
    consumedTokens: "",
    usageAction: "set",
    applyParallelLimit: false,
    applyTokenLimit: false,
    applyConsumedTokens: false,
    applyExpiresAt: false,
  };
}

function validEmail(value: string) {
  return /^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(value);
}

function errorText(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

function anchorInvalid(period: ShareTokenPeriod, anchorAtMs: number | undefined) {
  return ANCHORED_PERIODS.has(period) && (anchorAtMs == null || !Number.isFinite(anchorAtMs)
    || anchorAtMs > Math.floor(Date.now() / 60_000) * 60_000);
}

/**
 * The exact policy a valid USD draft saves. Preview and save both use it, so
 * the previewed intent is byte-for-byte what is applied.
 */
function usdDraftPolicy(draft: Pick<GrantDraft, "parallelLimit" | "tokenLimit" | "tokenPeriod" | "tokenPeriodAnchor" | "expiresAt">): ShareUserPolicy | undefined {
  const amount = parseUsdLimit(draft.tokenLimit);
  if (!amount.ok) return undefined;
  const parallelLimit = draft.parallelLimit.trim() ? Number(draft.parallelLimit) : undefined;
  const expiresAt = draft.expiresAt ? new Date(draft.expiresAt).getTime() : undefined;
  const tokenPeriodAnchorAtMs = ANCHORED_PERIODS.has(draft.tokenPeriod) ? parseUtcDateTime(draft.tokenPeriodAnchor) : undefined;
  if ((parallelLimit != null && (!Number.isInteger(parallelLimit) || parallelLimit < 1))
    || (expiresAt != null && !Number.isFinite(expiresAt)) || anchorInvalid(draft.tokenPeriod, tokenPeriodAnchorAtMs)) return undefined;
  return {
    parallelLimit,
    tokenLimit: undefined,
    valueQuota: { limitUsdMicros: amount.micros, tariffPolicyId: "api-equivalent-v1" },
    tokenPeriod: draft.tokenPeriod,
    tokenPeriodAnchorAtMs,
    expiresAt,
  };
}

export function ShareUserGrantsEditor({
  value,
  ownerEmail,
  defaultPolicy,
  supportedPeriods,
  t,
  disabled,
  shareId,
  configRevision,
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
  configRevision?: number;
  usageEdits?: ShareUserUsageEditMap;
  onUsageEditsChange?: (value: ShareUserUsageEditMap) => void;
  onChange: (value: ShareUserGrantMap) => void;
}) {
  const normalizedOwner = ownerEmail.trim().toLowerCase();
  const [accountPreview, setAccountPreview] = React.useState<AccountResetPreview>();
  const { items: valueStatuses, stale: valueStatusesStale } = useShareValueQuotaSnapshot(shareId, true);
  const setValueStatuses = (items: ValueQuotaStatus[]) => publishValueQuotaStatuses(shareId, items);
  const [previewIntent, setPreviewIntent] = React.useState("");
  const [previewBusy, setPreviewBusy] = React.useState(false);
  const [draftCycle, setDraftCycle] = React.useState<ShareUserGrant["accountQuotaCycle"]>();
  const [batchCycle, setBatchCycle] = React.useState<ShareUserGrant["accountQuotaCycle"]>();
  const accountCapability = accountPreview?.accountResetCapability ?? Object.values(value).find((grant) => grant.accountResetCapability)?.accountResetCapability;
  const accountCycle = accountPreview?.accountQuotaCycle ?? Object.values(value).find((grant) => grant.accountQuotaCycle)?.accountQuotaCycle;
  const confirmAccountPreview = async (policies: Record<string, ShareUserPolicy>, intent: string) => {
    if (!shareId || configRevision == null) throw new Error(t("dashboard.userLimit.accountReset.saveFirst"));
    if (previewIntent === intent && accountPreview?.accountResetCapability.syncStatus === "ready") return true;
    setPreviewBusy(true);
    try {
      const preview = await previewShareAccountReset(shareId, configRevision, policies);
      setAccountPreview(preview);
      if (preview.accountResetCapability.syncStatus !== "ready" || preview.rows.some((row) => row.reason)) {
        throw new Error(preview.accountResetCapability.reason || t("dashboard.userLimit.accountReset.notReady"));
      }
      setPreviewIntent(intent);
      return false;
    } finally { setPreviewBusy(false); }
  };
  const [editingEmail, setEditingEmail] = React.useState<string | null>(null);
  const [grantDraft, setGrantDraft] = React.useState<GrantDraft | null>(null);
  const [selecting, setSelecting] = React.useState(false);
  const [selectedEmails, setSelectedEmails] = React.useState<Set<string>>(new Set());
  const [batchDraft, setBatchDraft] = React.useState<BatchGrantDraft | null>(null);
  const [error, setError] = React.useState("");
  const [liveRows, setLiveRows] = React.useState<ShareUserLimitStatusRow[] | null>(null);
  const {
    breakdown,
    breakdownRevision,
    onExpand,
    refresh: refreshBreakdown,
    errors: breakdownErrors,
    loaded: breakdownLoaded,
  } = useShareUserUsageBreakdown(shareId);
  const supported = new Set<ShareTokenPeriod>(
    supportedPeriods?.length
      ? supportedPeriods
      : ["lifetime", "day", "week", "calendarMonth"],
  );
  const periods = ([
    { key: "accountReset", label: t("dashboard.userLimit.periodAccountReset") },
    { key: "lifetime", label: t("dashboard.userLimit.periodLifetime") },
    { key: "day", label: t("dashboard.userLimit.periodDay") },
    { key: "week", label: t("dashboard.userLimit.periodWeek") },
    { key: "sevenDays", label: t("dashboard.userLimit.periodSevenDays") },
    { key: "calendarMonth", label: t("dashboard.userLimit.periodMonth") },
    { key: "thirtyDays", label: t("dashboard.userLimit.periodThirtyDays") },
  ] satisfies Array<{ key: ShareTokenPeriod; label: string }>).filter((period) =>
    supported.has(period.key) || (period.key === "accountReset" && Object.values(value).some((grant) => grant.policy.tokenPeriod === "accountReset")),
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

  const batchSourceGrants = () => {
    const source = { ...value };
    for (const grant of grants) source[grant.email] ??= grant;
    return source;
  };
  /** One patch builder for preview and save, so both see identical policies. */
  const batchPatchFor = (draft: BatchGrantDraft): ShareUserPolicyBatchPatch => {
    const parallelLimit = draft.parallelLimit.trim() ? Number(draft.parallelLimit) : undefined;
    const amount = draft.quotaUnit === "usdEquivalent" ? parseUsdLimit(draft.tokenLimit) : undefined;
    const tokenLimit = draft.quotaUnit === "tokens" && draft.tokenLimit.trim() ? millionsInputToTokens(draft.tokenLimit) : undefined;
    const expiresAt = draft.expiresAt ? new Date(draft.expiresAt).getTime() : undefined;
    const periodAnchorAtMs = ANCHORED_PERIODS.has(draft.tokenPeriod) ? parseUtcDateTime(draft.tokenPeriodAnchor) : undefined;
    return {
      ...(draft.applyParallelLimit ? { parallelLimit: { value: parallelLimit } } : {}),
      ...(draft.applyTokenLimit ? { tokenLimit: {
        value: tokenLimit ?? undefined,
        ...(amount?.ok ? { valueQuota: { limitUsdMicros: amount.micros, tariffPolicyId: "api-equivalent-v1" as const } } : {}),
        period: draft.tokenPeriod,
        periodAnchorAtMs,
      } } : {}),
      ...(draft.applyExpiresAt ? { expiresAt: { value: expiresAt } } : {}),
    };
  };
  const singleUsdPolicies = (() => {
    if (grantDraft?.quotaUnit !== "usdEquivalent") return undefined;
    const email = grantDraft.email.trim().toLowerCase();
    const policy = validEmail(email) ? usdDraftPolicy(grantDraft) : undefined;
    return policy ? { [email]: policy } : undefined;
  })();
  const batchUsdPolicies = (() => {
    if (!batchDraft?.applyTokenLimit || batchDraft.quotaUnit !== "usdEquivalent" || !selectedEditableEmails.size) return undefined;
    const patch = batchPatchFor(batchDraft);
    const parallel = patch.parallelLimit?.value;
    if (!patch.tokenLimit?.valueQuota || anchorInvalid(batchDraft.tokenPeriod, patch.tokenLimit.periodAnchorAtMs)
      || (parallel != null && (!Number.isInteger(parallel) || parallel < 1))
      || (patch.expiresAt?.value != null && !Number.isFinite(patch.expiresAt.value))) return undefined;
    // Preview the requested policy, not what staging keeps in force.
    const source = batchSourceGrants();
    return Object.fromEntries([...selectedEditableEmails].sort().map((email) => [email, patchedUserPolicy(source[email], patch)]));
  })();
  const dialogOpen = !!grantDraft || !!batchDraft;
  const hasUsdGrant = grants.some((grant) => grant.policy.valueQuota || grant.pendingValueQuota);
  // Every grant with a USD limit in force, including one whose next USD limit is scheduled.
  const usdGrants = grants.flatMap((grant) => {
    const enforced = valueQuotaView(grant, grant.email, valueStatuses)?.enforced;
    return enforced ? [{ grant, enforced }] : [];
  });
  const usdSelected = grantDraft?.quotaUnit === "usdEquivalent" || (!!batchDraft?.applyTokenLimit && batchDraft.quotaUnit === "usdEquivalent");
  // Check once when a quota dialog opens (to gate `$`); poll only while USD matters.
  const readiness = useValueQuotaReadiness(shareId, configRevision, { enabled: dialogOpen, poll: dialogOpen && (usdSelected || hasUsdGrant) });
  const singlePreview = useValueQuotaPreview(shareId, configRevision, readiness.key, singleUsdPolicies);
  const batchPreview = useValueQuotaPreview(shareId, configRevision, readiness.key, batchUsdPolicies);
  const needsAcknowledgement = (state: { preview?: ValueQuotaPreview }) =>
    !!state.preview && !valuePreviewBlocker(state.preview) && valuePreviewNeedsAcknowledgement(state.preview);
  const [applying, setApplying] = React.useState(false);
  const [amountAttempted, setAmountAttempted] = React.useState(false);
  /**
   * True when the previewed policy may be applied now. A preview that needs
   * acknowledgement and was not on screen yet stops here, so the next press
   * reads "Confirm & apply". Errors and blockers render in the preview line.
   */
  const resolveValuePreview = async (state: typeof singlePreview) => {
    const alreadyShown = !!state.preview;
    setApplying(true);
    let preview: ValueQuotaPreview | undefined;
    try { preview = await state.fetchNow(); } finally { setApplying(false); }
    if (!preview || valuePreviewBlocker(preview)) return false;
    return alreadyShown || !valuePreviewNeedsAcknowledgement(preview);
  };
  const limitError = (unit: GrantDraft["quotaUnit"], raw: string) => {
    if (unit !== "usdEquivalent") return undefined;
    const parsed = parseUsdLimit(raw);
    if (parsed.ok || (parsed.error === "empty" && !amountAttempted)) return undefined;
    return t(usdAmountErrorKey(parsed.error));
  };
  const [usdSectionOpen, setUsdSectionOpen] = React.useState(false);
  const [correctionEmail, setCorrectionEmail] = React.useState<string | null>(null);
  const usdSectionRef = React.useRef<HTMLDetailsElement>(null);
  const fieldId = React.useId();

  const openAdd = () => {
    setPreviewIntent("");
    setAmountAttempted(false);
    setDraftCycle(accountCycle);
    setEditingEmail(null);
    setError("");
    setGrantDraft(makeDraft("", { ...defaultPolicy, tokenPeriod: accountCapability?.syncStatus === "ready" && supported.has("accountReset") ? "accountReset" : defaultPolicy.tokenPeriod }));
  };

  const openEdit = (grant: ShareUserGrant) => {
    if (shareMarketManagedEmails.has(grant.email)) return;
    setPreviewIntent("");
    setAmountAttempted(false);
    setDraftCycle(grant.accountQuotaCycle);
    setEditingEmail(grant.email);
    setError("");
    // Edit the policy the owner asked for; a scheduled one is that intent.
    setGrantDraft({
      ...makeDraft(grant.email, grant.pendingValueQuota?.policy ?? grant.policy),
      ...usageEditForGrant(grant, usageEdits),
    });
  };

  const exitSelecting = () => {
    setSelecting(false);
    setSelectedEmails(new Set());
  };

  const openBatchEdit = () => {
    setPreviewIntent("");
    if (!selecting) {
      setSelecting(true);
      return;
    }
    const firstSelected = grants.find((grant) => selectedEditableEmails.has(grant.email));
    if (!firstSelected) return;
    setBatchCycle(firstSelected.accountQuotaCycle);
    setError("");
    setAmountAttempted(false);
    setBatchDraft(makeBatchDraft(firstSelected));
  };

  const applyGrants = (userGrants: ShareUserGrantMap) => onChange(userGrants);

  const save = async () => {
    if (!grantDraft) return;
    if (grantDraft.tokenPeriod === "accountReset" && grantDraft.quotaUnit !== "usdEquivalent") {
      try {
        if (!await confirmAccountPreview({ [grantDraft.email]: { tokenPeriod: "accountReset",
          tokenLimit: millionsInputToTokens(grantDraft.tokenLimit) ?? undefined } }, JSON.stringify(grantDraft))) return;
      } catch (error) { setError(errorText(error)); return; }
    }
    const email = grantDraft.email.trim().toLowerCase();
    const parallelLimit = grantDraft.parallelLimit.trim()
      ? Number(grantDraft.parallelLimit)
      : undefined;
    if (grantDraft.quotaUnit === "usdEquivalent" && !parseUsdLimit(grantDraft.tokenLimit).ok) {
      setAmountAttempted(true); setError(""); return;
    }
    const hasTokenLimit = grantDraft.quotaUnit === "tokens" && !!grantDraft.tokenLimit.trim();
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
      grantDraft.quotaUnit === "tokens" &&
      grantDraft.usageAction === "set" &&
      !!grantDraft.consumedTokens.trim() &&
      (consumedTokens == null ||
        consumedTokens < 0 ||
        (grantDraft.tokenPeriod !== "accountReset" && consumedTokens < observedTokens));
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
    const usdPolicy = grantDraft.quotaUnit === "usdEquivalent" ? usdDraftPolicy(grantDraft) : undefined;
    if (grantDraft.quotaUnit === "usdEquivalent" && !usdPolicy) {
      // Never fall back to a Token policy while `$` is selected.
      setError(t("dashboard.userLimit.invalidPolicy"));
      return;
    }
    const target: ShareUserPolicy = usdPolicy ?? {
      parallelLimit,
      tokenLimit: tokenLimit ?? undefined,
      tokenPeriod: grantDraft.tokenPeriod,
      tokenPeriodAnchorAtMs,
      expiresAt,
    };
    const next: ShareUserGrant = stageUserPolicy(ordinaryShareUserGrant(email, normalizedOwner, previous, target), target,
      previous && !reuseMarketTombstone ? previous.policy : target);
    if (target.valueQuota) {
      setError("");
      if (!await resolveValuePreview(singlePreview)) return;
    }
    if (grantDraft.tokenPeriod === "accountReset") {
      next.accountQuotaCycle = previous?.policy.tokenPeriod === "accountReset" ? draftCycle : accountCycle;
      next.accountResetCapability = accountCapability;
    }
    if (grantDraft.quotaUnit === "tokens" && grantDraft.usageAction === "set" && consumedTokens != null) {
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
    } else if (grantDraft.quotaUnit === "tokens" && grantDraft.usageAction === "clear") {
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
      if (grantDraft.quotaUnit === "usdEquivalent") delete nextEdits[email];
      if (grantDraft.quotaUnit === "tokens" && grantDraft.usageAction === "set" && consumedTokens != null) {
        nextEdits[email] = {
          action: "set",
          targetTokens: consumedTokens,
          expectedGrantRevision: previous?.revision,
          expectedAccountCycle: grantDraft.tokenPeriod === "accountReset" ? next.accountQuotaCycle : undefined,
          period: grantDraft.tokenPeriod,
          anchorAtMs: tokenPeriodAnchorAtMs,
          source: usageEdits[editingEmail || email]?.source ?? "manual",
        };
      } else if (grantDraft.quotaUnit === "tokens" && grantDraft.usageAction === "clear") {
        nextEdits[email] = {
          action: "clear",
          expectedGrantRevision: previous?.revision,
          expectedAccountCycle: grantDraft.tokenPeriod === "accountReset" ? next.accountQuotaCycle : undefined,
          period: grantDraft.tokenPeriod,
          anchorAtMs: tokenPeriodAnchorAtMs,
        };
      } else if (grantDraft.quotaUnit === "usdEquivalent" || grantDraft.usageAction === "unchanged") {
        delete nextEdits[email];
      }
      onUsageEditsChange(nextEdits);
    }
    setGrantDraft(null);
  };

  const saveBatch = async () => {
    if (!batchDraft || selectedEditableEmails.size === 0) return;
    const previewGrants = grants.filter((grant) => selectedEditableEmails.has(grant.email));
    if (batchDraft.quotaUnit === "tokens" && (batchDraft.applyTokenLimit || batchDraft.applyConsumedTokens) && previewGrants.some((grant) =>
      (batchDraft.applyTokenLimit ? batchDraft.tokenPeriod : grant.policy.tokenPeriod) === "accountReset")) {
      try {
        const policies = Object.fromEntries(previewGrants.filter((grant) =>
          (batchDraft.applyTokenLimit ? batchDraft.tokenPeriod : grant.policy.tokenPeriod) === "accountReset")
          .map((grant) => [grant.email, { ...grant.policy, tokenPeriod: "accountReset" as const,
            tokenLimit: batchDraft.applyTokenLimit ? millionsInputToTokens(batchDraft.tokenLimit) ?? undefined : grant.policy.tokenLimit }]));
        if (!await confirmAccountPreview(policies, JSON.stringify([batchDraft, [...selectedEditableEmails].sort()]))) return;
      } catch (error) { setError(errorText(error)); return; }
    }
    const parallelLimit = batchDraft.parallelLimit.trim()
      ? Number(batchDraft.parallelLimit)
      : undefined;
    const amount = batchDraft.quotaUnit === "usdEquivalent" ? parseUsdLimit(batchDraft.tokenLimit) : undefined;
    const valueQuota = amount?.ok ? amount.micros : undefined;
    if (batchDraft.applyTokenLimit && amount && !amount.ok) { setAmountAttempted(true); setError(""); return; }
    if (batchDraft.applyConsumedTokens && previewGrants.some((g) => g.policy.valueQuota) && !batchDraft.applyTokenLimit) {
      setError(t("dashboard.userLimit.value.usageEditUnavailable")); return;
    }
    const hasTokenLimit = batchDraft.quotaUnit === "tokens" && !!batchDraft.tokenLimit.trim();
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
      !!batchDraft.consumedTokens.trim() &&
      (consumedTokens == null ||
        consumedTokens < 0 ||
        (!(batchDraft.applyTokenLimit ? batchDraft.tokenPeriod === "accountReset" : selectedGrants.every((grant) => grant.policy.tokenPeriod === "accountReset")) && consumedTokens < usageFloor));
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

    const nextGrants = applyShareUserPolicyBatch(batchSourceGrants(), selectedEditableEmails, batchPatchFor(batchDraft));
    if (batchDraft.applyTokenLimit && valueQuota) {
      setError("");
      if (!await resolveValuePreview(batchPreview)) return;
    }
    if (batchDraft.applyConsumedTokens && !valueQuota) {
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
    if (batchDraft.applyTokenLimit && batchDraft.tokenPeriod === "accountReset") {
      for (const email of selectedEditableEmails) {
        if (nextGrants[email]) nextGrants[email] = { ...nextGrants[email], accountQuotaCycle: batchCycle ?? accountCycle, accountResetCapability: accountCapability };
      }
    }
    applyGrants(nextGrants);
    if (onUsageEditsChange && (batchDraft.applyTokenLimit || batchDraft.applyConsumedTokens)) {
      const nextEdits: ShareUserUsageEditMap = { ...usageEdits };
      for (const grant of selectedGrants) {
        const email = grant.email.trim().toLowerCase();
        if (nextGrants[email]?.policy.valueQuota || (batchDraft.applyTokenLimit && batchDraft.quotaUnit === "usdEquivalent")) { delete nextEdits[email]; continue; }
        const period = batchDraft.applyTokenLimit ? batchDraft.tokenPeriod : grant.policy.tokenPeriod;
        if (batchDraft.applyConsumedTokens && batchDraft.usageAction === "set" && consumedTokens != null) {
          nextEdits[email] = {
            action: "set",
            targetTokens: consumedTokens,
            expectedGrantRevision: grant.revision,
            expectedAccountCycle: period === "accountReset" ? batchCycle ?? accountCycle : undefined,
            period: batchDraft.applyTokenLimit ? batchDraft.tokenPeriod : grant.policy.tokenPeriod,
            anchorAtMs: batchDraft.applyTokenLimit
              ? tokenPeriodAnchorAtMs
              : grant.policy.tokenPeriodAnchorAtMs,
            source: usageEdits[email]?.source ?? "manual",
          };
        } else if (batchDraft.applyConsumedTokens && batchDraft.usageAction === "clear") {
          nextEdits[email] = {
            action: "clear",
            expectedGrantRevision: grant.revision,
            expectedAccountCycle: period === "accountReset" ? batchCycle ?? accountCycle : undefined,
            period: batchDraft.applyTokenLimit ? batchDraft.tokenPeriod : grant.policy.tokenPeriod,
            anchorAtMs: batchDraft.applyTokenLimit
              ? tokenPeriodAnchorAtMs
              : grant.policy.tokenPeriodAnchorAtMs,
          };
        } else if (batchDraft.applyTokenLimit && nextEdits[email]) {
          // Preserve an explicit calibration already staged in this draft.
          nextEdits[email] = { ...nextEdits[email], period: batchDraft.tokenPeriod, anchorAtMs: tokenPeriodAnchorAtMs };
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

  const previewPanel = previewIntent && accountPreview ? <div role="status" className="text-xs">
    {t("dashboard.userLimit.accountReset.preview")}
    {accountPreview.rows.map((row) => <div key={row.email}>{row.email}: {row.currentTokens ?? "—"} → {row.expectedTokens ?? "—"}{row.wouldExceed ? ` · ${t("dashboard.userLimit.accountReset.exceeded")}` : ""}</div>)}
  </div> : null;
  return (
    <div className="grid gap-3">
      <div className="text-xs text-muted-foreground" role="status">
        {t("dashboard.userLimit.periodAccountReset")}: {accountCapability?.syncStatus ?? t("dashboard.userLimit.accountReset.pending")}{accountCapability?.reason ? ` · ${accountCapability.reason}` : ""}
        {accountCycle ? <div>{accountCycle.bucketId} · {new Date(accountCycle.startsAtMs).toISOString()} → {accountCycle.endsAtMs ? new Date(accountCycle.endsAtMs).toISOString() : t("dashboard.userLimit.accountReset.noEnd")}
          {" · "}{accountCycle.basis} · {t("dashboard.userLimit.accountReset.observed")} {new Date(accountCycle.observedAtMs).toISOString()}</div> : null}
        {t("dashboard.userLimit.accountReset.caveat")}
        {accountPreview?.ledgerHealth?.capacityWarning ? <div role="status">quota_ledger_capacity_warning ({accountPreview.ledgerHealth.revisionCount}/{accountPreview.ledgerHealth.admissionLimit})</div> : null}
      </div>
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
          valueQuotaStatuses={valueStatuses}
          valueQuotaStale={valueStatusesStale}
          onValueReconcile={!disabled && shareId && configRevision != null ? (email) => {
            setUsdSectionOpen(true);
            setCorrectionEmail(email.trim().toLowerCase());
            window.requestAnimationFrame(() => usdSectionRef.current?.scrollIntoView({ block: "nearest", behavior: "smooth" }));
          } : undefined}
          grants={grants}
          t={t}
          breakdown={shareId ? breakdown : undefined}
          breakdownRevision={shareId ? breakdownRevision : undefined}
          breakdownErrors={shareId ? breakdownErrors : undefined}
          breakdownLoaded={shareId ? breakdownLoaded : undefined}
          onExpand={shareId ? onExpand : undefined}
          onPriceSaved={shareId ? refreshBreakdown : undefined}
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

      {usdGrants.length ? <details ref={usdSectionRef} open={usdSectionOpen} onToggle={(event) => setUsdSectionOpen(event.currentTarget.open)} className="rounded-md border p-3 text-sm">
        <summary className="cursor-pointer">{t("dashboard.userLimit.value.status")}</summary>
        {usdGrants.map(({ grant, enforced }) => {
          const status = enforced.status;
          const email = grant.email.trim().toLowerCase();
          const editable = !!status && status.meteringStatus !== "scheduled" && !!shareId && configRevision != null && !protectedEmails.has(grant.email) && !disabled;
          return <div className="border-t py-2" key={grant.email}>
            <div className="flex min-w-0 flex-wrap items-baseline gap-x-2">
              <span className="min-w-0 break-all font-medium">{grant.email}</span>
              <span className="font-mono text-xs">{formatQuotaPicos(status?.usedPicos)} / {formatUsdLimit(enforced.limitMicros)}</span>
              {status?.meteringStatus === "blocked" ? <span className="text-xs font-medium text-danger">{t("dashboard.userLimit.value.needsReconcile")}</span> : null}
            </div>
            {editable && status ? <ShareValueUsageAdjustment shareId={shareId!} revision={configRevision!} grant={grant} status={status} t={t}
              onUpdated={setValueStatuses} open={correctionEmail === email} onOpenChange={(open) => setCorrectionEmail(open ? email : null)} />
              : !status ? <p className="text-xs text-muted-foreground">{t("dashboard.userLimit.value.unsynced")}</p> : null}
          </div>;
        })}
      </details> : null}
      <Modal.Backdrop
        isOpen={!!grantDraft}
        onOpenChange={(open) => !open && setGrantDraft(null)}
        className="z-[70]"
      >
          <Modal.Container placement="center" className="z-[70]">
            <Modal.Dialog className="share-edit-surface light w-[min(620px,calc(100vw-2rem))] max-w-none bg-background text-foreground">
              <Modal.CloseTrigger className="share-quota-close" />
              <Modal.Header>
                <Modal.Heading>{editingEmail ? t("dashboard.userLimit.edit") : t("dashboard.userLimit.add")}</Modal.Heading>
              </Modal.Header>
              <Modal.Body className="grid gap-4 !text-slate-900 sm:grid-cols-2">
                <div className="grid gap-1.5 sm:col-span-2">
                  <label htmlFor={`${fieldId}-email`} className="mono-label text-muted-foreground">Email</label>
                  <Input id={`${fieldId}-email`} type="email" value={grantDraft?.email || ""} disabled={!!editingEmail} onChange={(event) => grantDraft && setGrantDraft({ ...grantDraft, email: event.target.value })} />
                </div>
                <div className="grid content-start gap-1.5">
                  <label htmlFor={`${fieldId}-parallel`} className="mono-label text-muted-foreground">{t("dashboard.field.parallelLimit")}</label>
                  <Input id={`${fieldId}-parallel`} type="number" min={1} placeholder={t("common.unlimited")} value={grantDraft?.parallelLimit || ""} onChange={(event) => grantDraft && setGrantDraft({ ...grantDraft, parallelLimit: event.target.value })} />
                </div>
                <div className="grid content-start gap-1.5">
                  <span className="mono-label text-muted-foreground">{t("dashboard.userLimit.value.unit")}</span>
                  <QuotaUnitSwitch t={t} value={grantDraft?.quotaUnit || "tokens"} usdDisabled={readiness.unavailable} readinessId={`${fieldId}-readiness`}
                    onChange={(unit) => { if (!grantDraft || unit === grantDraft.quotaUnit) return; setAmountAttempted(false); setGrantDraft({ ...grantDraft, quotaUnit: unit, tokenLimit: "", usageAction: "unchanged", consumedTokens: "" }); }} />
                  <ValueQuotaReadiness state={readiness} t={t} id={`${fieldId}-readiness`} />
                </div>
                <div className="grid gap-1.5 sm:col-span-2">
                  <QuotaLimitField id={`${fieldId}-limit`} t={t} unit={grantDraft?.quotaUnit || "tokens"} value={grantDraft?.tokenLimit || ""}
                    error={limitError(grantDraft?.quotaUnit || "tokens", grantDraft?.tokenLimit || "")}
                    onChange={(tokenLimit) => grantDraft && setGrantDraft({ ...grantDraft, tokenLimit })} />
                  {grantDraft?.quotaUnit === "usdEquivalent" ? <ValuePreviewLine state={singlePreview} t={t} /> : null}
                </div>
                <div className={grantDraft?.quotaUnit === "usdEquivalent" ? "hidden" : "grid gap-1.5 sm:col-span-2"}>
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
                  <Select aria-label={t("dashboard.userLimit.period")} selectedKey={grantDraft?.tokenPeriod || "lifetime"} onSelectionChange={(key) => {
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
                  <label htmlFor={`${fieldId}-expires`} className="mono-label text-muted-foreground">{t("dashboard.field.expiresAt")}</label>
                  <Input id={`${fieldId}-expires`} type="datetime-local" value={grantDraft?.expiresAt || ""} onChange={(event) => grantDraft && setGrantDraft({ ...grantDraft, expiresAt: event.target.value })} />
                </div>
                {error ? <p className="text-sm text-red-600 sm:col-span-2">{error}</p> : null}
              </Modal.Body>
              {previewPanel}
              <Modal.Footer>
                <Button variant="outline" onClick={() => setGrantDraft(null)}>{t("common.cancel")}</Button>
                <Button variant="primary" isDisabled={previewBusy || applying} aria-busy={applying || undefined} onClick={() => void save()}>
                  {grantDraft?.quotaUnit === "usdEquivalent" && needsAcknowledgement(singlePreview) ? t("dashboard.userLimit.value.confirmApply") : t("common.apply")}
                </Button>
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
          <Modal.Dialog className="share-edit-surface light w-[min(620px,calc(100vw-2rem))] max-w-none bg-background text-foreground">
            <Modal.CloseTrigger className="share-quota-close" />
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
                      {t(batchDraft?.quotaUnit === "usdEquivalent" ? "dashboard.userLimit.value.limit" : "dashboard.field.tokenLimit")}
                    </span>
                  </Checkbox.Content>
                </Checkbox>
                <QuotaUnitSwitch t={t} disabled={!batchDraft?.applyTokenLimit} value={batchDraft?.quotaUnit || "tokens"} usdDisabled={readiness.unavailable} readinessId={`${fieldId}-batch-readiness`}
                  onChange={(unit) => { if (!batchDraft || unit === batchDraft.quotaUnit) return; setAmountAttempted(false); setBatchDraft({ ...batchDraft, quotaUnit: unit, tokenLimit: "", applyConsumedTokens: false }); }} />
                {batchDraft?.applyTokenLimit ? <ValueQuotaReadiness state={readiness} t={t} id={`${fieldId}-batch-readiness`} /> : null}
                <QuotaLimitField id={`${fieldId}-batch-limit`} labelHidden t={t} disabled={!batchDraft?.applyTokenLimit} unit={batchDraft?.quotaUnit || "tokens"}
                  value={batchDraft?.tokenLimit || ""} error={batchDraft?.applyTokenLimit ? limitError(batchDraft.quotaUnit, batchDraft.tokenLimit) : undefined}
                  onChange={(tokenLimit) => batchDraft && setBatchDraft({ ...batchDraft, tokenLimit })} />
                {batchDraft?.applyTokenLimit && batchDraft.quotaUnit === "usdEquivalent" ? <ValuePreviewLine state={batchPreview} t={t} /> : null}
              </div>
              <div className="grid gap-1.5">
                <span className="mono-label text-muted-foreground">
                  {t("dashboard.userLimit.period")}
                </span>
                <Select
                  aria-label={t("dashboard.userLimit.period")}
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
            {previewPanel}
            <Modal.Footer>
              <Button variant="outline" onClick={() => {
                setBatchDraft(null);
                setError("");
              }}>
                {t("common.cancel")}
              </Button>
              <Button
                variant="primary"
                isDisabled={previewBusy || applying || (!!batchDraft &&
                  !batchDraft.applyParallelLimit &&
                  !batchDraft.applyTokenLimit &&
                  !batchDraft.applyConsumedTokens &&
                  !batchDraft.applyExpiresAt)}
                onClick={saveBatch}
              >
                {batchDraft?.applyTokenLimit && batchDraft.quotaUnit === "usdEquivalent" && needsAcknowledgement(batchPreview)
                  ? t("dashboard.userLimit.value.confirmApply")
                  : t("dashboard.userLimit.batchApply", { count: selectedEditableEmails.size })}
              </Button>
            </Modal.Footer>
          </Modal.Dialog>
        </Modal.Container>
      </Modal.Backdrop>
    </div>
  );
}
