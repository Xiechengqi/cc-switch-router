"use client";
/*
 * The Client market's "Rental audit" tab: the shared owner audit workspace
 * configured for Client rentals. Only the Router owner sees the tab
 * (`session.canAuditClientMarket`, from router_owner_email, not isAdmin), and
 * the server re-checks every read.
 */
import * as React from "react";
import { useAuth } from "@/components/auth/auth-provider";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { RentalAuditWorkspace, type RentalAuditConfig } from "@/components/market/rental-audit-workspace";
import { AUDIT_ATTENTION, AUDIT_STATUSES, getRentalAudit, getRentalAuditDetail, type AuditDetail, type AuditRecord } from "@/lib/client-rental-audit";
import { rentalAuditMessagesEn } from "@/lib/client-rental-audit-messages";
import { formatWalletUnits } from "@/lib/market-wallet-money";
import type { MessageKey } from "@/lib/i18n";

/** The Router owner, or nobody. Shared by the market tabs and this panel. */
export function useRentalAuditActor() {
  const { session } = useAuth();
  return session?.authenticated && session.canAuditClientMarket === true ? session.user?.id : undefined;
}

/** Remounted per account so no previous owner's emails or money remain. */
export function ClientRentalAuditPanel({ onTerminationOpen, revision = 0 }: { onTerminationOpen: (id: string) => void; revision?: number }) {
  const actor = useRentalAuditActor();
  return actor ? <ClientAudit key={actor} onTerminationOpen={onTerminationOpen} revision={revision} /> : null;
}

function ClientAudit({ onTerminationOpen, revision }: { onTerminationOpen: (id: string) => void; revision: number }) {
  const { locale, t } = useLocaleText();
  const config = React.useMemo<RentalAuditConfig<AuditRecord, AuditDetail>>(() => {
    const time = (v: string | null) => v ? new Date(v).toLocaleString(locale) : "—";
    const status = (value: string) => t(`rentalAudit.status.${value}` as MessageKey);
    const attention = (value: string) => t(`rentalAudit.attention.${value}` as MessageKey);
    return {
      scope: "client",
      statuses: AUDIT_STATUSES,
      attention: AUDIT_ATTENTION,
      statusLabel: status,
      attentionLabel: attention,
      eventLabel: (action) => {
        const key = `rentalAudit.event.${action}`;
        return t(key in rentalAuditMessagesEn ? key as MessageKey : "rentalAudit.event.updated");
      },
      searchHint: t("rentalAudit.searchHint"),
      subjectHeader: "Client",
      subject: (record) => record.clientSubdomain || record.installationId || record.jobId || record.id,
      list: getRentalAudit,
      detail: getRentalAuditDetail,
      badges: (record) => {
        const badges = [status(record.status)];
        if (record.cleanupStatus !== "not_required") badges.push(t(`rentalAudit.cleanup.${record.cleanupStatus}`));
        if (record.attention !== "none" && !badges.includes(attention(record.attention))) badges.push(attention(record.attention));
        return badges;
      },
      facts: (detail) => {
        const record = detail.record;
        const price = record.cyclePriceMinor !== null
          ? t("rentalAudit.monthlyPrice", { price: formatWalletUnits((BigInt(record.cyclePriceMinor) * 86_400n).toString(), "$", locale) })
          : record.dailyRateMinor !== null
            ? t("rentalAudit.dailyPrice", { price: formatWalletUnits((BigInt(record.dailyRateMinor) * 86_400n).toString(), "$", locale) })
            : record.pricingModel === "free" ? t("rentalAudit.freePrice") : null;
        const renewal = record.renewalPolicy === "automatic" || record.renewalPolicy === "manual"
          ? t(`rentalAudit.renewal.${record.renewalPolicy}`) : null;
        return [
          [t("rentalAudit.orderId"), record.id], [t("rentalAudit.jobId"), record.jobId], [t("rentalAudit.installationId"), record.installationId],
          [t("rentalAudit.contractId"), record.contractId], [t("rentalAudit.client"), record.clientSubdomain], ["Host", record.hostId],
          [t("rentalAudit.buyer"), record.buyerEmail], [t("rentalAudit.provider"), record.providerEmail], ["IP", record.hostIp ? `${record.hostIp}${record.hostPort ? `:${record.hostPort}` : ""}` : null],
          [t("rentalAudit.created"), time(record.createdAt)], [t("rentalAudit.activated"), time(record.activatedAt)], [t("rentalAudit.ended"), time(record.endedAt)],
          [t("rentalAudit.cleanupAt"), time(record.cleanupConfirmedAt)], [t("rentalAudit.offer"), record.offerRevision?.toString() ?? null],
          [t("rentalAudit.price"), price], [t("rentalAudit.renewal"), renewal],
          [t("rentalAudit.trial"), record.trialSecondsRemaining === null ? "—" : `${detail.trialConsumedSeconds} / ${record.trialSecondsRemaining}`],
        ];
      },
      note: (detail) => `${t(detail.record.provenance === "order_snapshot" ? "rentalAudit.snapshot" : "rentalAudit.historical")}${!detail.record.hostPresent && detail.record.hostId ? ` · ${t("rentalAudit.removed")}` : ""}`,
    };
  }, [locale, t]);
  return <RentalAuditWorkspace config={config} onTerminationOpen={onTerminationOpen} revision={revision} />;
}
