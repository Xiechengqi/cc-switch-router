"use client";

import * as React from "react";
import { Alert, Button, Tooltip } from "@heroui/react";
import { Info } from "lucide-react";
import type { TFn } from "@/components/dashboard/share-dashboard-utils";
import type { ShareEditView, ShareView } from "@/lib/types";

export function ReadOnlyField({
  label,
  value,
}: {
  label: string;
  value: React.ReactNode;
}) {
  return (
    <div className="grid gap-1">
      <span className="text-xs font-medium uppercase tracking-wide text-slate-500">{label}</span>
      <div className="rounded-lg border border-slate-200 bg-slate-50 px-3 py-2 text-sm text-slate-900">{value}</div>
    </div>
  );
}

export function ShareEditHint({ label }: { label: string }) {
  return (
    <Tooltip>
      <Tooltip.Trigger>
        <Button
          isIconOnly
          size="sm"
          variant="ghost"
          className="h-5 w-5 min-w-5 text-slate-400"
          aria-label={label}
        >
          <Info className="h-3.5 w-3.5" />
        </Button>
      </Tooltip.Trigger>
      <Tooltip.Content className="max-w-xs text-xs leading-5">{label}</Tooltip.Content>
    </Tooltip>
  );
}

export function ShareEditSection({
  title,
  hint,
  actions,
  children,
}: {
  title: string;
  hint?: string;
  actions?: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <section className="grid gap-3 rounded-2xl border border-slate-200/80 bg-white p-4">
      <div className="flex min-w-0 items-center justify-between gap-2">
        <h3 className="flex min-w-0 items-center gap-1 text-[11px] font-semibold uppercase tracking-[0.08em] text-slate-500">
          <span className="truncate">{title}</span>
          {hint ? <ShareEditHint label={hint} /> : null}
        </h3>
        {actions}
      </div>
      {children}
    </section>
  );
}

export function shareEditPendingLabel(edit: ShareEditView, t: TFn) {
  if (edit.patch.managedGrant?.action === "upsert") {
    return t("dashboard.shareMarketGrantPending");
  }
  if (edit.patch.managedGrant?.action === "revoke") {
    return t("dashboard.shareMarketRevokePending");
  }
  return t("dashboard.pendingApply");
}

export function ShareEditStatusBanner({ share, t }: { share: ShareView; t: TFn }) {
  const edit = share.activeEdit;
  if (!share.canManage || !edit) return null;
  if (edit.status === "pending") {
    return (
      <Alert status="warning" className="!text-slate-900">
        {shareEditPendingLabel(edit, t)}
      </Alert>
    );
  }
  if (edit.status === "rejected") {
    return (
      <Alert status="danger" className="!text-slate-900">
        {edit.errorMessage || t("dashboard.applyFailedFallback")}
      </Alert>
    );
  }
  return null;
}

export function ReadOnlyChipList({ items }: { items: string[] }) {
  if (!items.length) {
    return <span className="text-muted-foreground">—</span>;
  }
  return (
    <div className="flex flex-wrap gap-1.5">
      {items.map((item) => (
        <span
          key={item}
          className="inline-flex max-w-full items-center rounded-full border border-primary/20 bg-primary/10 px-2.5 py-1 text-xs font-medium text-primary"
        >
          <span className="min-w-0 truncate">{item}</span>
        </span>
      ))}
    </div>
  );
}
