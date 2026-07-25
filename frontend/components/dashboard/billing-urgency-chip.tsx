"use client";

import * as React from "react";
import { Button } from "@heroui/react";
import { Clock3 } from "lucide-react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import {
  billingDeadlineTarget,
  billingUrgencyTier,
  formatBillingAbsoluteDate,
  formatBillingCountdown,
} from "@/lib/billing-urgency";
import type { ClientMarketBilling } from "@/lib/types";
import { cn } from "@/lib/utils";

export function BillingUrgencyChip({
  billing,
  compact = false,
  showPayButton = false,
  onPay,
}: {
  billing?: ClientMarketBilling;
  /** Shorter market-row copy */
  compact?: boolean;
  showPayButton?: boolean;
  onPay?: () => void;
}) {
  const { locale, t } = useLocaleText();
  const [, tick] = React.useState(0);
  const [softOpen, setSoftOpen] = React.useState(false);
  const softRef = React.useRef<HTMLDivElement | null>(null);
  const tier = billingUrgencyTier(billing);
  const target = billing ? billingDeadlineTarget(billing) : undefined;

  React.useEffect(() => {
    if (!tier || tier === "silent") return;
    const timer = window.setInterval(() => tick((value) => value + 1), 30_000);
    return () => window.clearInterval(timer);
  }, [tier]);

  React.useEffect(() => {
    if (!softOpen) return;
    const onPointerDown = (event: MouseEvent) => {
      if (!softRef.current?.contains(event.target as Node)) setSoftOpen(false);
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") setSoftOpen(false);
    };
    document.addEventListener("mousedown", onPointerDown);
    document.addEventListener("keydown", onKeyDown);
    return () => {
      document.removeEventListener("mousedown", onPointerDown);
      document.removeEventListener("keydown", onKeyDown);
    };
  }, [softOpen]);

  if (!billing || !tier || tier === "silent" || !target) return null;

  const countdown = formatBillingCountdown(target, locale);
  const absolute = formatBillingAbsoluteDate(
    billing.status === "payment_due" ? billing.paymentDeadline || billing.currentPeriodEnd : billing.currentPeriodEnd,
    locale,
  );
  const softLabel = compact
    ? t("billing.billSoonCompact", { countdown })
    : t("billing.billSoon", { countdown });
  const urgentLabel = compact
    ? t("billing.deadlineCompact", { countdown })
    : t("billing.deadlineLabel", { countdown });

  if (tier === "soft") {
    return (
      <div ref={softRef} className="relative inline-flex" data-no-row-drawer>
        <button
          type="button"
          onClick={(event) => {
            event.stopPropagation();
            setSoftOpen((open) => !open);
          }}
          className="inline-flex h-6 shrink-0 items-center gap-1 rounded-full border border-slate-200 bg-white px-2.5 text-[11px] font-medium text-slate-600 transition-colors hover:border-slate-300 hover:bg-slate-50 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary/30"
          aria-expanded={softOpen}
          title={absolute || undefined}
        >
          <Clock3 className="h-3 w-3 shrink-0 text-slate-400" aria-hidden />
          <span>{softLabel}</span>
        </button>
        {softOpen ? (
          <div
            role="dialog"
            className="absolute left-0 top-full z-40 mt-1.5 w-max max-w-[16rem] rounded-lg border border-border bg-white p-3 text-xs shadow-md"
            onClick={(event) => event.stopPropagation()}
          >
            <div className="grid gap-1">
              <span className="text-muted-foreground">{t("billing.paidThroughHint", { date: absolute || "-" })}</span>
              <span className="text-muted-foreground">{t("billing.nextBillHint", { countdown })}</span>
            </div>
          </div>
        ) : null}
      </div>
    );
  }

  return (
    <div className="inline-flex min-w-0 flex-wrap items-center gap-1.5" data-no-row-drawer onClick={(event) => event.stopPropagation()}>
      <span
        className={cn(
          "inline-flex h-6 shrink-0 items-center gap-1 rounded-full border px-2.5 text-[11px] font-medium",
          "border-amber-300/80 text-amber-800",
        )}
        title={absolute || undefined}
      >
        <Clock3 className="h-3 w-3 shrink-0" aria-hidden />
        <span>{urgentLabel}</span>
      </span>
      {showPayButton && onPay ? (
        <Button
          size="sm"
          variant="primary"
          className="h-6 px-2.5 text-[11px]"
          onClick={(event) => {
            event.stopPropagation();
            onPay();
          }}
        >
          {t("billing.goPay")}
        </Button>
      ) : null}
    </div>
  );
}
