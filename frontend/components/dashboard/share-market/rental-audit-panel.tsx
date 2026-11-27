"use client";
/*
 * The Share market's "Rental audit" tab: the shared owner audit workspace
 * (`components/market/rental-audit-workspace.tsx`) configured for Share seat
 * rentals. Only the Router owner sees the tab; the server re-checks every read.
 */
import * as React from "react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { useRentalAuditActor } from "@/components/dashboard/client-market/rental-audit-panel";
import { RentalAuditWorkspace, type RentalAuditConfig } from "@/components/market/rental-audit-workspace";
import { rentalAuditMessagesEn } from "@/lib/client-rental-audit-messages";
import type { MessageKey } from "@/lib/i18n";
import { formatWalletUnits } from "@/lib/market-wallet-money";
import {
  SHARE_AUDIT_ATTENTION,
  SHARE_AUDIT_STATUSES,
  getShareRentalAudit,
  getShareRentalAuditDetail,
  type ShareAuditDetail,
  type ShareAuditRecord,
} from "@/lib/share-rental-audit";

/** USD micro-units as exact dollars, trimming only trailing zeros. */
function usdMicros(value: string) {
  const micros = BigInt(value);
  const whole = micros / 1_000_000n;
  const fraction = (micros % 1_000_000n).toString().padStart(6, "0").replace(/0+$/, "");
  return `$${whole}${fraction ? `.${fraction}` : ""}`;
}
const pair = (first: string | null, second: string | null, format: (v: string) => string = (v) => v) =>
  first === null && second === null ? null : `${first === null ? "—" : format(first)} / ${second === null ? "—" : format(second)}`;

/** Remounted per account so no previous owner's emails or money remain. */
export function ShareRentalAuditPanel({ onTerminationOpen, revision = 0 }: { onTerminationOpen: (id: string) => void; revision?: number }) {
  const actor = useRentalAuditActor();
  return actor ? <ShareAudit key={actor} onTerminationOpen={onTerminationOpen} revision={revision} /> : null;
}

function ShareAudit({ onTerminationOpen, revision }: { onTerminationOpen: (id: string) => void; revision: number }) {
  const { locale, t } = useLocaleText();
  const config = React.useMemo<RentalAuditConfig<ShareAuditRecord, ShareAuditDetail>>(() => {
    const time = (v: string | null) => v ? new Date(v).toLocaleString(locale) : "—";
    const status = (value: string) => t(`shareRentalAudit.status.${value}` as MessageKey);
    const attention = (value: string) => t(`shareRentalAudit.attention.${value}` as MessageKey);
    return {
      scope: "share",
      statuses: SHARE_AUDIT_STATUSES,
      attention: SHARE_AUDIT_ATTENTION,
      statusLabel: status,
      attentionLabel: attention,
      // The server allowlists event names (unknown ones arrive as "updated");
      // termination actions reuse the Client audit's names and labels.
      eventLabel: (action) => {
        const share = `shareRentalAudit.event.${action}`;
        const client = `rentalAudit.event.${action}`;
        return t((share in rentalAuditMessagesEn ? share : client in rentalAuditMessagesEn ? client : "rentalAudit.event.updated") as MessageKey);
      },
      searchHint: t("shareRentalAudit.searchHint"),
      subjectHeader: t("shareRentalAudit.share"),
      subject: (record) => {
        const share = record.shareName || record.shareSubdomain || record.shareId;
        return record.seatPosition !== null ? `${share} · ${t("shareRentalAudit.seatN", { position: record.seatPosition })}` : share;
      },
      list: getShareRentalAudit,
      detail: getShareRentalAuditDetail,
      badges: (record) => {
        const badges = [status(record.status)];
        if (record.attention !== "none") badges.push(attention(record.attention));
        return badges;
      },
      facts: (detail) => {
        const record = detail.record;
        const price = record.dailyRateMinor !== null
          ? t("rentalAudit.dailyPrice", { price: formatWalletUnits((BigInt(record.dailyRateMinor) * 86_400n).toString(), "$", locale) })
          : record.pricingModel === "free" ? t("rentalAudit.freePrice") : null;
        const renewal = record.renewalPolicy === "automatic" || record.renewalPolicy === "manual"
          ? t(`rentalAudit.renewal.${record.renewalPolicy}`) : null;
        return [
          [t("rentalAudit.orderId"), record.id], [t("shareRentalAudit.shareId"), record.shareId],
          [t("shareRentalAudit.share"), record.shareName], [t("shareRentalAudit.subdomain"), record.shareSubdomain],
          [t("shareRentalAudit.appType"), record.appType], [t("shareRentalAudit.seat"), record.seatPosition !== null ? t("shareRentalAudit.seatN", { position: record.seatPosition }) : record.seatId],
          [t("shareRentalAudit.installationId"), record.installationId], [t("rentalAudit.contractId"), record.contractId],
          [t("rentalAudit.buyer"), record.buyerEmail], [t("rentalAudit.provider"), record.providerEmail],
          [t("rentalAudit.created"), time(record.createdAt)], [t("rentalAudit.activated"), time(record.activatedAt)],
          [t("shareRentalAudit.expires"), time(record.expiresAt)], [t("rentalAudit.ended"), time(record.endedAt)],
          [t("shareRentalAudit.releaseReason"), record.releaseReason],
          [t("rentalAudit.offer"), record.offerRevision?.toString() ?? null],
          [t("rentalAudit.price"), price], [t("rentalAudit.renewal"), renewal],
        ];
      },
      note: (detail) => `${t(detail.record.provenance === "order_snapshot" ? "rentalAudit.snapshot" : "shareRentalAudit.historical")}${!detail.record.sharePresent ? ` · ${t("shareRentalAudit.removed")}` : ""}`,
      extra: (detail) => {
        const { trial, settlement } = detail;
        if (!trial && !settlement) return null;
        const rows: Array<[string, string | null]> = [
          ...(trial ? [
            [t("shareRentalAudit.trialPhase"), trial.phase],
            [t("shareRentalAudit.trialTokens"), pair(trial.tokenAllowance, trial.tokenBudget)],
            [t("shareRentalAudit.trialValue"), pair(trial.valueAllowanceUsdMicros, trial.valueBudgetUsdMicros, usdMicros)],
          ] as Array<[string, string | null]> : []),
          ...(settlement ? [
            [t("shareRentalAudit.settlementStatus"), settlement.status],
            [t("shareRentalAudit.responsibility"), settlement.responsibility],
            [t("shareRentalAudit.cutoff"), time(settlement.cutoffAt)],
            [t("rentalAudit.returnedUnits"), settlement.returnedUnits === null ? null : formatWalletUnits(settlement.returnedUnits, "$", locale)],
            [t("rentalAudit.compensationUnits"), settlement.compensationUnits === null ? null : formatWalletUnits(settlement.compensationUnits, "$", locale)],
          ] as Array<[string, string | null]> : []),
        ];
        return (
          <section className="grid gap-3 rounded-lg border border-border p-3">
            <h3 className="text-sm font-semibold">{t(trial ? "shareRentalAudit.trial" : "shareRentalAudit.settlement")}{trial && settlement ? ` · ${t("shareRentalAudit.settlement")}` : ""}</h3>
            <dl className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
              {rows.map(([label, value]) => (
                <div key={label}><dt className="text-xs text-muted-foreground">{label}</dt><dd className="mt-0.5 break-all text-sm tabular-nums">{value || "—"}</dd></div>
              ))}
            </dl>
          </section>
        );
      },
    };
  }, [locale, t]);
  return <RentalAuditWorkspace config={config} onTerminationOpen={onTerminationOpen} revision={revision} />;
}
