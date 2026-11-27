"use client";

import * as React from "react";
import Link from "next/link";
import { Button, toast } from "@heroui/react";
import { useAuth } from "@/components/auth/auth-provider";
import { ExpandableRecordTable, RecordDetailList } from "@/components/common/expandable-record-table";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { ClientTerminationDialog } from "@/components/market/client-termination-dialog";
import { ClientTerminationRecord } from "@/components/market/client-terminations-panel";
import type { ClientTerminationAction } from "@/lib/client-termination";
import { ApiError } from "@/lib/api";
import { clientMarketMineHref } from "@/lib/dashboard-nav";
import { formatUsdMoney } from "@/lib/market-money";
import { cancelOnlineReclaim, clientDisplayName, onlineMarketError, onlineRequest, type OnlineSellerItem as SellerItem } from "@/lib/online-client-market";
import { ClientReclaimDialog, type ClientReclaimTarget } from "./client-market/client-reclaim-dialog";
import { AccountInlineLoading, AccountSectionError } from "./account-section";
import type { AccountMarketRole } from "./account-share-section";
import { endReasonKey, onlineStatusKey } from "./client-market/host-utils";
import { useBilledTime } from "./client-market/online-shop";
import { formatWalletUnits } from "@/lib/market-wallet-money";
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
  const [reclaiming, setReclaiming] = React.useState<ClientReclaimTarget | null>(null);
  const [cancelling, setCancelling] = React.useState(false);
  const billedTime = useBilledTime();
  const date = (sec: number | null | undefined) => sec ? new Date(sec * 1000).toLocaleString(locale) : "—";
  const status = (entry: SellerItem) => entry.reclaimEffectiveAt != null && ["starting", "active"].includes(entry.state)
    ? t("clientMarket.reclaim.scheduledShort", { date: date(entry.reclaimEffectiveAt) })
    : entry.paused && entry.state !== "released" ? t("online.paused") : t(onlineStatusKey(entry.state));
  async function cancelReclaim(entry: SellerItem) {
    if (cancelling) return;
    setCancelling(true);
    try {
      const result = await cancelOnlineReclaim(entry.id);
      toast.success(t(result?.renewalRestored ? "clientMarket.reclaim.cancelledRenewal" : "clientMarket.reclaim.cancelled"));
      setAttempt(v => v + 1);
    } catch (cause) {
      toast.danger(onlineMarketError(cause, t));
    } finally {
      setCancelling(false);
    }
  }
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
      rowLabel={clientDisplayName}
      ariaLabel={t("clientMarket.title")}
      focusKey={focusKey}
      columns={[
        { id: "client", header: t("online.clientColumn"), cell: entry => clientDisplayName(entry) },
        { id: "status", header: t("account.market.col.status"), cell: status },
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
                  value: date(entry.expiresAt),
                },
                ...(billedTime(entry) ? [{ label: t("clientRentals.billedHours"), value: billedTime(entry) }] : []),
                ...(entry.incomeUnits != null ? [{ label: t("clientMarket.seller.income"), value: formatWalletUnits(entry.incomeUnits, "$", locale) }] : []),
                ...(entry.endReason ? [{ label: t("clientRentals.reason"), value: t(endReasonKey(entry.endReason)) }] : []),
              ]}
            />
            {entry.reclaimEffectiveAt != null || entry.reclaimable ? (
              <div className="flex flex-wrap gap-2">
                {entry.reclaimEffectiveAt != null && entry.reclaimEffectiveAt * 1000 > Date.now() ? (
                  <Button size="sm" variant="outline" isDisabled={cancelling} onPress={() => void cancelReclaim(entry)}>
                    {t("clientMarket.reclaim.cancel")}
                  </Button>
                ) : null}
                {entry.reclaimable && entry.reclaimMode !== "immediate" ? (
                  <Button size="sm" variant="outline" onPress={() => setReclaiming({ leaseId: entry.id, name: clientDisplayName(entry), escalate: entry.reclaimEffectiveAt != null, starting: entry.state === "starting" })}>
                    {t(entry.reclaimEffectiveAt != null ? "clientMarket.reclaim.escalate" : "clientMarket.reclaim.action")}
                  </Button>
                ) : null}
              </div>
            ) : null}
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
    <ClientReclaimDialog target={reclaiming} onClose={() => setReclaiming(null)} onDone={() => { toast.success(t("clientMarket.reclaim.done")); setAttempt(v => v + 1); }} />
    <ClientTerminationDialog terminationId={selection?.id || null} initialRole="provider" initialAction={selection?.action}
      onClose={() => setSelection(null)} onChanged={() => { void terminations.reload(); setAttempt(v => v + 1); }} />
  </div>;
}
