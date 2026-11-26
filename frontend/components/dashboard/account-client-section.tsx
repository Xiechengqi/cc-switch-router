"use client";

import * as React from "react";
import Link from "next/link";
import { Button } from "@heroui/react";
import { useAuth } from "@/components/auth/auth-provider";
import { ExpandableRecordTable, RecordDetailList } from "@/components/common/expandable-record-table";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { ClientTerminationDialog } from "@/components/market/client-termination-dialog";
import { ClientTerminationRecord } from "@/components/market/client-terminations-panel";
import type { ClientTerminationAction } from "@/lib/client-termination";
import { ApiError } from "@/lib/api";
import { clientMarketMineHref } from "@/lib/dashboard-nav";
import { formatUsdMoney } from "@/lib/market-money";
import { onlineRequest, type OnlineSellerItem as SellerItem } from "@/lib/online-client-market";
import { AccountInlineLoading, AccountSectionError } from "./account-section";
import type { AccountMarketRole } from "./account-share-section";
import { onlineStatusKey } from "./client-market/host-utils";
import { ClientRentalRecords } from "./client-market/rental-records";
import { useAccountClientTerminations } from "./use-account-client-terminations";

export function AccountClientSection({ role, focusKey }: { role: AccountMarketRole; focusKey?: string | null }) {
  const { session } = useAuth();
  const actor = session?.authenticated ? session.user?.id : undefined;
  if (role === "user") return <ClientRentalRecords focusKey={focusKey} />;
  return actor ? <SellerInventory key={actor} actor={actor} focusKey={focusKey} /> : null;
}

function SellerInventory({ actor, focusKey }: { actor: string; focusKey?: string | null }) {
  const { t, locale } = useLocaleText();
  const [items, setItems] = React.useState<SellerItem[] | null>(null);
  const [error, setError] = React.useState("");
  const [attempt, setAttempt] = React.useState(0);
  const [selection, setSelection] = React.useState<{ id: string; action?: ClientTerminationAction } | null>(null);
  const terminations = useAccountClientTerminations(actor, "provider", focusKey);
  React.useEffect(() => {
    const controller = new AbortController();
    let timer: ReturnType<typeof setTimeout>;
    async function refresh() {
      try {
        const response = await onlineRequest<{ items: SellerItem[] }>("seller-inventory", "GET", undefined, controller.signal);
        if (!controller.signal.aborted) { setItems(response.items); setError(""); }
      } catch (cause) {
        if (!controller.signal.aborted) {
          if (cause instanceof ApiError && (cause.status === 401 || cause.status === 403)) setItems(null);
          setError(cause instanceof Error ? cause.message : String(cause));
        }
      } finally {
        if (!controller.signal.aborted) timer = setTimeout(refresh, 20_000);
      }
    }
    void refresh();
    return () => { controller.abort(); clearTimeout(timer); };
  }, [attempt]);

  const byId = new Map(terminations.items.map((item) => [item.id, item]));
  return <div className="grid min-w-0 gap-4">
    {error && <AccountSectionError message={error} onRetry={() => setAttempt(v => v + 1)} />}
    {!items && !error && <AccountInlineLoading />}
    {items && <ExpandableRecordTable
      rows={items}
      rowKey={entry => entry.id}
      rowLabel={entry => entry.label}
      ariaLabel={t("clientMarket.title")}
      focusKey={focusKey}
      columns={[
        { id: "client", header: t("online.clientColumn"), cell: entry => entry.label },
        { id: "status", header: t("account.market.col.status"), cell: entry => t(onlineStatusKey(entry.state)) },
        { id: "renter", header: t("account.market.col.renter"), cell: entry => entry.tenantEmail || t("account.market.notRented"), minWidth: "md" },
      ]}
      empty={<p className="py-6 text-center text-sm text-muted-foreground">{t("account.client.providerEmpty")}</p>}
      renderDetail={(entry) => {
        const termination = entry.terminationId ? byId.get(entry.terminationId) : null;
        return (
          <div className="grid gap-3">
            <RecordDetailList
              items={[
                {
                  label: t("account.client.offer"),
                  value: entry.cyclePriceMinor
                    ? t("rentalAudit.monthlyPrice", { price: formatUsdMoney(entry.cyclePriceMinor, locale) })
                    : t("shareMarket.free"),
                },
                {
                  label: t("account.market.col.renter"),
                  value: entry.tenantEmail || t("account.market.notRented"),
                },
                {
                  label: t("account.market.col.term"),
                  value: entry.expiresAt ? new Date(entry.expiresAt * 1000).toLocaleString(locale) : "—",
                },
              ]}
            />
            {entry.terminationId && !termination && (
              <Button variant="outline" onPress={() => setSelection({ id: entry.terminationId! })}>
                {t("online.exitRecord")}
              </Button>
            )}
            {termination && (
              <ClientTerminationRecord
                item={termination}
                role="provider"
                onOpen={(item, action) => setSelection({ id: item.id, action })}
              />
            )}
            {entry.listingId && (
              <Link
                className="justify-self-start text-sm text-accent hover:underline"
                href={clientMarketMineHref(entry.listingId)}
              >
                {t("account.client.manageInMarket")}
              </Link>
            )}
          </div>
        );
      }}
    />}
    {terminations.error && <AccountSectionError message={t("account.market.terminationsUnavailable")} onRetry={() => void terminations.reload()} />}
    {terminations.nextCursor && <Button size="sm" variant="ghost" onPress={() => void terminations.more()}>{t("account.market.earlierTerminations")}</Button>}
    <ClientTerminationDialog terminationId={selection?.id || null} initialRole="provider" initialAction={selection?.action}
      onClose={() => setSelection(null)} onChanged={() => { void terminations.reload(); setAttempt(v => v + 1); }} />
  </div>;
}
