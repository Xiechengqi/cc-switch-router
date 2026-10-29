"use client";

import * as React from "react";
import { Button } from "@heroui/react";
import { Loader2, RefreshCw, type LucideIcon } from "lucide-react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { cn } from "@/lib/utils";

/**
 * The heading every account section opens with: the section name as the page
 * `h2`, one line saying what it is for, and page-level actions on the right.
 * Panels inside a section use `h3`.
 */
export function AccountSectionHeader({
  title,
  hint,
  icon: Icon,
  actions,
}: {
  title: React.ReactNode;
  hint?: React.ReactNode;
  icon?: LucideIcon;
  actions?: React.ReactNode;
}) {
  return (
    <div className="flex min-w-0 flex-wrap items-start justify-between gap-3">
      <div className="min-w-0">
        <h2 className="flex items-center gap-2 text-base font-semibold text-foreground">
          {Icon ? <Icon className="h-4 w-4 shrink-0 text-muted-foreground" aria-hidden /> : null}
          {title}
        </h2>
        {hint ? <p className="mt-0.5 max-w-2xl text-sm text-muted-foreground">{hint}</p> : null}
      </div>
      {actions ? <div className="flex flex-wrap items-center gap-2">{actions}</div> : null}
    </div>
  );
}

/** A load or save failure, with a retry when the caller can offer one. */
export function AccountSectionError({
  message,
  onRetry,
  className,
}: {
  message: React.ReactNode;
  onRetry?: () => void;
  className?: string;
}) {
  const { t } = useLocaleText();
  return (
    <div
      role="alert"
      className={cn(
        "flex min-w-0 flex-wrap items-center gap-3 rounded-lg border border-rose-200 bg-rose-50 px-3 py-2 text-sm text-rose-700",
        className,
      )}
    >
      <span className="min-w-0 flex-1 break-words">{message}</span>
      {onRetry ? (
        <Button size="sm" variant="outline" onPress={onRetry}>
          <RefreshCw className="h-4 w-4" aria-hidden />
          {t("common.retry")}
        </Button>
      ) : null}
    </div>
  );
}

/** Loading text for a panel inside a section that is already on screen. */
export function AccountInlineLoading({ label, className }: { label?: React.ReactNode; className?: string }) {
  const { t } = useLocaleText();
  return (
    <p role="status" className={cn("flex items-center gap-2 py-4 text-sm text-muted-foreground", className)}>
      <Loader2 className="h-4 w-4 animate-spin motion-reduce:animate-none" aria-hidden />
      {label ?? t("common.loading")}
    </p>
  );
}
