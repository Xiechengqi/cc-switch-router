"use client";

import { Button } from "@heroui/react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import type { ClientTerminationHostSummary } from "@/lib/client-termination";

export function actionableHostTermination(
  summary?: ClientTerminationHostSummary,
) {
  return (
    !!summary &&
    summary.actionRequiredCount > 0 &&
    ["provider_response", "owner_decision", "cleanup_failed"].includes(
      summary.attention,
    )
  );
}

export function ClientTerminationHostAction({
  summary,
  onOpen,
  describedBy,
}: {
  summary?: ClientTerminationHostSummary;
  onOpen?: (id: string) => void;
  /** Id of the element naming the Client, so the action has per-card context. */
  describedBy?: string;
}) {
  const { locale, t } = useLocaleText();
  if (!summary || !onOpen || !actionableHostTermination(summary)) return null;
  return (
    <div className="grid justify-items-end gap-0.5">
      <Button
        size="sm"
        variant="danger"
        className="min-h-11 shrink-0 whitespace-nowrap px-2 text-xs focus-visible:ring-2 focus-visible:ring-accent focus-visible:ring-offset-2"
        aria-describedby={describedBy}
        onPress={() => onOpen(summary.terminationId)}
      >
        {t(
          summary.attention === "owner_decision"
            ? "clientTermination.rowAdjudicate"
            : "clientTermination.rowHandle",
        )}
        {summary.actionRequiredCount > 1
          ? ` · ${summary.actionRequiredCount}`
          : ""}
      </Button>
      {summary.responseDeadlineAt &&
        summary.attention === "provider_response" && (
          <time
            dateTime={summary.responseDeadlineAt}
            className="text-xs text-muted-foreground"
            title={t("clientTermination.responseDeadline")}
          >
            {t("clientTermination.respondBy", {
              time: new Intl.DateTimeFormat(locale, {
                month: "short",
                day: "numeric",
                hour: "2-digit",
                minute: "2-digit",
              }).format(new Date(summary.responseDeadlineAt)),
            })}
          </time>
        )}
    </div>
  );
}
