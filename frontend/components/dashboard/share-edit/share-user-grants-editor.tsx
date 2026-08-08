"use client";

import { Button, Input, ListBox, Modal, Select, Tooltip } from "@heroui/react";
import { Info, Pencil, Plus, Trash2 } from "lucide-react";
import * as React from "react";

import type { TFn } from "@/components/dashboard/share-dashboard-utils";
import type {
  ShareTokenPeriod,
  ShareUserGrant,
  ShareUserPolicy,
} from "@/lib/types";
import { routerShareMarketManagedEmails } from "@/lib/share-settings";
import { formatDateTime, formatNumber } from "@/lib/utils";
import type { PriceApp, ShareEditDraft } from "./share-edit-draft";

type GrantDraft = {
  email: string;
  parallelLimit: string;
  tokenLimit: string;
  tokenPeriod: ShareTokenPeriod;
  tokenPeriodAnchor: string;
  expiresAt: string;
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
    tokenLimit: policy.tokenLimit == null ? "" : String(policy.tokenLimit),
    tokenPeriod: policy.tokenPeriod || "lifetime",
    tokenPeriodAnchor: toUtcDateTime(policy.tokenPeriodAnchorAtMs),
    expiresAt: toLocalDateTime(policy.expiresAt),
  };
}

function validEmail(value: string) {
  return /^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(value);
}

export function ShareUserGrantsEditor({
  draft,
  activeShareApps,
  ownerEmail,
  defaultPolicy,
  supportedPeriods,
  t,
  onDraftChange,
}: {
  draft: ShareEditDraft;
  activeShareApps: PriceApp[];
  ownerEmail: string;
  defaultPolicy: ShareUserPolicy;
  supportedPeriods?: ShareTokenPeriod[];
  t: TFn;
  onDraftChange: (updater: (current: ShareEditDraft) => ShareEditDraft) => void;
}) {
  const normalizedOwner = ownerEmail.trim().toLowerCase();
  const [editingEmail, setEditingEmail] = React.useState<string | null>(null);
  const [grantDraft, setGrantDraft] = React.useState<GrantDraft | null>(null);
  const [error, setError] = React.useState("");
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
  const tokenMarketEmails = new Set(draft.selectedMarketEmails);
  const shareMarketManagedEmails = routerShareMarketManagedEmails(draft.userGrants);
  const protectedEmails = new Set([
    ...tokenMarketEmails,
    ...shareMarketManagedEmails,
  ]);
  const visibleEmails = new Set([
    normalizedOwner,
    ...Object.values(draft.shareToEmailsByApp).flat(),
    ...protectedEmails,
  ]);
  const grants = Array.from(visibleEmails)
    .filter(Boolean)
    .map((email) => draft.userGrants[email] ?? ({
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

  const openAdd = () => {
    setEditingEmail(null);
    setError("");
    setGrantDraft(makeDraft("", defaultPolicy));
  };

  const openEdit = (grant: ShareUserGrant) => {
    if (shareMarketManagedEmails.has(grant.email)) return;
    setEditingEmail(grant.email);
    setError("");
    setGrantDraft(makeDraft(grant.email, grant.policy));
  };

  const applyGrants = (userGrants: ShareEditDraft["userGrants"]) => {
    const emails = Object.values(userGrants)
      .filter((grant) => grant.active !== false && grant.role === "shareto")
      .filter((grant) => !tokenMarketEmails.has(grant.email))
      .map((grant) => grant.email)
      .sort();
    onDraftChange((current) => ({
      ...current,
      userGrants,
      shareToEmailsByApp: Object.fromEntries(
        (["claude", "codex", "gemini"] as const).map((app) => [
          app,
          activeShareApps.includes(app) ? emails : current.shareToEmailsByApp[app],
        ]),
      ) as ShareEditDraft["shareToEmailsByApp"],
    }));
  };

  const save = () => {
    if (!grantDraft) return;
    const email = grantDraft.email.trim().toLowerCase();
    const parallelLimit = grantDraft.parallelLimit.trim()
      ? Number(grantDraft.parallelLimit)
      : undefined;
    const tokenLimit = grantDraft.tokenLimit.trim()
      ? Number(grantDraft.tokenLimit)
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
      setError(t("dashboard.userLimit.duplicateEmail"));
      return;
    }
    if (!editingEmail && draft.userGrants[email]?.active !== false && draft.userGrants[email]) {
      setError(t("dashboard.userLimit.duplicateEmail"));
      return;
    }
    if (
      (parallelLimit != null && (!Number.isInteger(parallelLimit) || parallelLimit < 1)) ||
      (tokenLimit != null && (!Number.isInteger(tokenLimit) || tokenLimit < 1)) ||
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
    const previous = draft.userGrants[editingEmail || email];
    const next: ShareUserGrant = {
      ...previous,
      email,
      role: email === normalizedOwner ? "owner" : "shareto",
      active: true,
      policy: {
        parallelLimit,
        tokenLimit,
        tokenPeriod: grantDraft.tokenPeriod,
        tokenPeriodAnchorAtMs,
        expiresAt,
      },
    };
    const userGrants = { ...draft.userGrants };
    if (editingEmail && editingEmail !== email) delete userGrants[editingEmail];
    userGrants[email] = next;
    applyGrants(userGrants);
    setGrantDraft(null);
  };

  const limit = (value?: number) =>
    value == null ? t("common.unlimited") : formatNumber(value);
  const expiry = (value?: number) =>
    value == null ? t("dashboard.permanent") : formatDateTime(value);

  return (
    <div className="grid gap-3">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
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
          <p className="mt-1 text-xs text-muted-foreground">{t("dashboard.userLimit.hint")}</p>
        </div>
        <Button size="sm" variant="outline" onClick={openAdd}>
          <Plus className="h-4 w-4" />
          {t("dashboard.userLimit.add")}
        </Button>
      </div>

      <div className="overflow-x-auto rounded-md border border-slate-200">
        <table className="w-full min-w-[720px] text-left text-sm">
          <thead className="border-b border-slate-200 bg-slate-50 text-xs text-slate-500">
            <tr>
              <th className="px-3 py-2 font-medium">Email</th>
              <th className="px-3 py-2 font-medium">{t("dashboard.field.parallelLimit")}</th>
              <th className="px-3 py-2 font-medium">Token</th>
              <th className="px-3 py-2 font-medium">{t("dashboard.field.expiresAt")}</th>
              <th className="w-20 px-3 py-2" />
            </tr>
          </thead>
          <tbody className="divide-y divide-slate-100">
            {grants.map((grant) => (
              <tr key={grant.email}>
                <td className="px-3 py-2">
                  <div className="flex items-center gap-2">
                    <span className="max-w-[250px] truncate">{grant.email}</span>
                    {grant.role === "owner" ? (
                      <span className="rounded bg-slate-100 px-1.5 py-0.5 text-[10px] font-semibold text-slate-600">Owner</span>
                    ) : null}
                    {shareMarketManagedEmails.has(grant.email) ? (
                      <span className="rounded bg-sky-50 px-1.5 py-0.5 text-[10px] font-semibold text-sky-700">Share Market</span>
                    ) : null}
                  </div>
                </td>
                <td className="px-3 py-2">{limit(grant.policy.parallelLimit)}</td>
                <td className="px-3 py-2">{limit(grant.policy.tokenLimit)} · {periodLabel[grant.policy.tokenPeriod]}</td>
                <td className="px-3 py-2">{expiry(grant.policy.expiresAt)}</td>
                <td className="px-3 py-2">
                  <div className="flex justify-end gap-1">
                    {!shareMarketManagedEmails.has(grant.email) ? (
                      <Button isIconOnly size="sm" variant="ghost" aria-label={t("common.edit")} onClick={() => openEdit(grant)}>
                        <Pencil className="h-4 w-4" />
                      </Button>
                    ) : null}
                    {grant.role !== "owner" && !protectedEmails.has(grant.email) ? (
                      <Button isIconOnly size="sm" variant="ghost" aria-label={t("common.delete")} onClick={() => {
                        const userGrants = { ...draft.userGrants };
                        delete userGrants[grant.email];
                        applyGrants(userGrants);
                      }}>
                        <Trash2 className="h-4 w-4 text-red-600" />
                      </Button>
                    ) : null}
                  </div>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>

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
                  <Input type="number" min={1} placeholder={t("common.unlimited")} value={grantDraft?.tokenLimit || ""} onChange={(event) => grantDraft && setGrantDraft({ ...grantDraft, tokenLimit: event.target.value })} />
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
                    <Select.Popover className="share-edit-popover light !bg-white !text-slate-900">
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
    </div>
  );
}
