"use client";

import * as React from "react";
import { Alert } from "@heroui/react";
import type { TFn } from "@/components/dashboard/share-dashboard-utils";
import type { ShareView } from "@/lib/types";

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

export function ShareEditSection({
  title,
  children,
}: {
  title: string;
  children: React.ReactNode;
}) {
  return (
    <section className="grid gap-3">
      <h3 className="text-xs font-semibold uppercase tracking-wide text-slate-500">{title}</h3>
      {children}
    </section>
  );
}

export function forSaleOptionLabel(forSale: "Yes" | "No" | "Free", t: TFn) {
  if (forSale === "Yes") return t("dashboard.yes");
  if (forSale === "Free") return t("dashboard.free");
  return t("dashboard.no");
}

export function ShareEditStatusBanner({ share, t }: { share: ShareView; t: TFn }) {
  const edit = share.activeEdit;
  if (!share.canManage || !edit) return null;
  if (edit.status === "pending") {
    return (
      <Alert status="warning" className="!text-slate-900">
        {t("dashboard.pendingApply")}
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
