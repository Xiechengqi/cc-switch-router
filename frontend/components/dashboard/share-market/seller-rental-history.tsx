"use client";
/*
 * "租用历史" at the bottom of Share "My listings": every ended rental of the
 * seller's seats, five per page, read from the server. Styled exactly like the
 * buyer's history in the catalog (`AccountShareRentals historyOnly`).
 */
import * as React from "react";
import { Button } from "@heroui/react";
import { ExpandableRecordTable, RecordDetailList, type RecordTableColumn } from "@/components/common/expandable-record-table";
import { RentalHistorySection } from "@/components/common/rental-history-section";
import { MarketPagination } from "@/components/dashboard/share-market/market-pagination";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { formatWalletUnits } from "@/lib/market-wallet-money";
import {
  getShareSellerHistory,
  shareSellerHistoryLabel,
  type ShareSellerHistoryPage,
  type ShareSellerHistoryRecord,
} from "@/lib/share-seller-history";

/** `revision` changes when a seat's rental changes, so a just-ended rental appears. */
export function ShareSellerRentalHistory({ revision = "" }: { revision?: string | number }) {
  const { locale, t } = useLocaleText();
  const [page, setPage] = React.useState<ShareSellerHistoryPage | null>(null);
  const [number, setNumber] = React.useState(1);
  const [failed, setFailed] = React.useState(false);
  const [busy, setBusy] = React.useState(true);
  const [retry, setRetry] = React.useState(0);

  React.useEffect(() => {
    const control = new AbortController();
    setBusy(true);
    getShareSellerHistory(number, control.signal)
      .then((next) => {
        if (control.signal.aborted) return;
        setPage(next);
        setFailed(false);
        // A shrinking history returns its last page; follow it.
        if (next.page !== number) setNumber(next.page);
      })
      .catch(() => {
        if (!control.signal.aborted) setFailed(true);
      })
      .finally(() => {
        if (!control.signal.aborted) setBusy(false);
      });
    return () => control.abort();
  }, [number, revision, retry]);

  const date = (value: string | null) =>
    value ? new Intl.DateTimeFormat(locale, { dateStyle: "short", timeStyle: "short" }).format(new Date(value)) : "—";
  const income = (row: ShareSellerHistoryRecord) => (row.incomeUnits != null ? formatWalletUnits(row.incomeUnits, "$", locale) : "—");
  const columns: RecordTableColumn<ShareSellerHistoryRecord>[] = [
    {
      id: "share",
      header: t("account.market.col.share"),
      cell: (row) => (
        <div className="grid min-w-0 gap-0.5">
          <span className="truncate font-medium" title={shareSellerHistoryLabel(row)}>{shareSellerHistoryLabel(row)}</span>
          <span className="truncate text-xs text-muted-foreground md:hidden">{row.buyerEmail || "—"}</span>
        </div>
      ),
    },
    {
      id: "tenant",
      header: t("clientMarket.seller.tenant"),
      minWidth: "md",
      cell: (row) => <span className="block truncate text-muted-foreground" title={row.buyerEmail || undefined}>{row.buyerEmail || "—"}</span>,
    },
    {
      id: "ended",
      header: t("rentalAudit.ended"),
      minWidth: "lg",
      className: "w-40",
      cell: (row) => <span className="text-muted-foreground">{date(row.endedAt)}</span>,
    },
    {
      id: "income",
      header: t("clientMarket.seller.income"),
      className: "w-24 sm:w-28",
      cell: (row) => <span className="block truncate tabular-nums">{income(row)}</span>,
    },
  ];

  // Sellers who never rented anything out see no empty block at the bottom.
  if (!failed && (!page || page.total === 0)) return null;
  return (
    <RentalHistorySection id="share-seller-history" title={t("account.share.history")} count={page?.total}>
      {failed ? (
        <div role="alert" className="flex flex-wrap items-center gap-2 text-sm text-destructive">
          {t("shareMarket.sellerHistory.error")}
          <Button size="sm" variant="ghost" className="h-9" onPress={() => setRetry((n) => n + 1)}>
            {t("common.retry")}
          </Button>
        </div>
      ) : null}
      {page ? (
        <>
          <div className={busy ? "opacity-60 transition-opacity" : undefined} aria-busy={busy}>
            <ExpandableRecordTable
              rows={page.items}
              rowKey={(row) => row.id}
              rowLabel={shareSellerHistoryLabel}
              columns={columns}
              ariaLabel={t("account.share.history")}
              renderDetail={(row) => (
                <RecordDetailList
                  items={[
                    { label: t("account.market.col.share"), value: <span className="break-all">{shareSellerHistoryLabel(row)}</span> },
                    { label: t("clientMarket.seller.tenant"), value: row.buyerEmail || "—" },
                    { label: t("rentalAudit.activated"), value: date(row.activatedAt) },
                    { label: t("rentalAudit.ended"), value: date(row.endedAt) },
                    { label: t("clientMarket.seller.income"), value: income(row) },
                  ]}
                />
              )}
              empty={<p className="text-sm text-muted-foreground">{t("account.share.historyEmpty")}</p>}
            />
          </div>
          <MarketPagination page={page.page} pageCount={page.totalPages} onPageChange={(next) => !busy && setNumber(next)} />
        </>
      ) : null}
    </RentalHistorySection>
  );
}
