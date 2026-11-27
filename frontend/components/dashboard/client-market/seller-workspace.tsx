"use client";
/*
 * The seller's side of the Client market: Clients listed for rent, Clients
 * rented out now, and past rentals. From here a seller changes terms, pauses
 * new orders, unlists, or reclaims a rented Client.
 */
import * as React from "react";
import { Button, Modal, toast } from "@heroui/react";
import { ClockAlert, Pause, PencilLine, Play, RotateCcw, Undo2 } from "lucide-react";
import { ExpandableRecordTable, RecordDetailList, type RecordTableColumn } from "@/components/common/expandable-record-table";
import { RentalHistorySection } from "@/components/common/rental-history-section";
import { MarketPagination } from "@/components/dashboard/share-market/market-pagination";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { ClientOnlineHeatmap } from "@/components/dashboard/client-online-heatmap";
import { ClientTerminationHostAction } from "@/components/market/client-termination-host-action";
import type { ClientTerminationHostSummary } from "@/lib/client-termination";
import type { MessageKey } from "@/lib/i18n";
import { formatUsdMoney } from "@/lib/market-money";
import {
  cancelOnlineReclaim,
  clientDisplayName,
  onlineMarketError,
  onlineRequest,
  setOnlineListingPaused,
  type OnlineListing,
  type OnlineListingSyncStatus,
  type OnlineSellerItem,
} from "@/lib/online-client-market";
import { ClientListingAddDialog } from "./client-listing-add-dialog";
import { ClientListingEditDialog } from "./client-listing-terms";
import { ClientReclaimDialog, type ClientReclaimTarget } from "./client-reclaim-dialog";
import { endReasonKey, onlineStatusKey } from "./host-utils";
import { ONLINE_CLIENT_CARD_GRID_CLASS, OnlineClientCard, OnlineSellingToolbar, useBilledTime } from "./online-shop";
import { formatWalletUnits } from "@/lib/market-wallet-money";

const HISTORY_PAGE_SIZE = 5;

function probeKey(code: string): MessageKey {
  switch (code) {
    case "tunnel_offline":
      return "online.routerDisconnected";
    case "upgrading":
      return "online.upgrading";
    case "switching":
      return "lease.switching";
    case "awaiting_receipt":
      return "lease.awaiting";
    case "status_mismatch":
      return "lease.mismatch";
    default:
      return "lease.controlUnavailable";
  }
}

const sellerItemRented = (item: OnlineSellerItem) =>
  item.state !== "released" && (item.startedAt != null || !!item.tenantEmail);

function ListingSyncDetail({ installationId, revision }: { installationId: string; revision?: number }) {
  const { t } = useLocaleText();
  const [status, setStatus] = React.useState<OnlineListingSyncStatus | null>(null);
  React.useEffect(() => {
    const abort = new AbortController();
    const load = async () => {
      try {
        const next = await onlineRequest<OnlineListingSyncStatus>(`installations/${encodeURIComponent(installationId)}/listing`, "GET", undefined, abort.signal);
        if (!abort.signal.aborted) setStatus(next);
      } catch {
        if (!abort.signal.aborted) setStatus(null);
      }
    };
    void load();
    const timer = setInterval(() => void load(), 20_000);
    return () => { abort.abort(); clearInterval(timer); };
  }, [installationId, revision]);
  if (!status || status.state !== "unlisted" || status.hostSyncState === "confirmed") return null;
  return <p role="status" className="text-xs text-muted-foreground">{t(status.hostSyncState === "after_rental" ? "online.hostSyncAfterRental" : status.hostSyncState === "waiting_reconnect" ? "online.hostSyncWaiting" : "online.hostSyncPending")}</p>;
}

export function SellerWorkspace({
  listings,
  items,
  loaded,
  addOpen,
  initialInstallation,
  onAddOpen,
  onAddClose,
  onChanged,
  terminations,
  onTerminationOpen,
}: {
  listings: OnlineListing[];
  items: OnlineSellerItem[];
  loaded: boolean;
  addOpen: boolean;
  initialInstallation: string | null;
  onAddOpen: () => void;
  onAddClose: () => void;
  onChanged: () => void;
  terminations: ReadonlyMap<string, ClientTerminationHostSummary>;
  onTerminationOpen: (id: string) => void;
}) {
  const { locale, t } = useLocaleText();
  const [query, setQuery] = React.useState("");
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState("");
  const [unlisting, setUnlisting] = React.useState<OnlineListing | null>(null);
  const [editing, setEditing] = React.useState<OnlineListing | null>(null);
  const [reclaiming, setReclaiming] = React.useState<ClientReclaimTarget | null>(null);
  const [historyPage, setHistoryPage] = React.useState(1);
  const listingById = React.useMemo(() => new Map(listings.map((item) => [item.installationId, item])), [listings]);
  const text = query.trim().toLowerCase();
  const matches = (item: OnlineSellerItem) =>
    !text || [item.label, item.title, item.tenantEmail].some((part) => (part || "").toLowerCase().includes(text));
  const history = items.filter((item) => item.state === "released" && matches(item));
  // Five per page, like the Share buyer's rental history.
  const historyPages = Math.max(1, Math.ceil(history.length / HISTORY_PAGE_SIZE));
  const shownHistoryPage = Math.min(historyPage, historyPages);
  const pagedHistory = history.slice((shownHistoryPage - 1) * HISTORY_PAGE_SIZE, shownHistoryPage * HISTORY_PAGE_SIZE);
  const rented = items.filter(sellerItemRented);
  const listed = items.filter((item) => item.state !== "released" && !sellerItemRented(item));
  const visibleListed = listed.filter(matches);
  const visibleRented = rented.filter(matches);
  const price = (minor: number) => (minor ? formatUsdMoney(minor, locale) + t("online.perMonth") : t("online.free"));
  const date = (sec: number | null | undefined) =>
    sec ? new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short" }).format(new Date(sec * 1000)) : "—";
  const nameOf = (item: OnlineSellerItem) => clientDisplayName(item);

  async function action(fn: () => Promise<void>) {
    if (busy) return;
    setBusy(true);
    setError("");
    try {
      await fn();
      onChanged();
    } catch (cause) {
      setError(onlineMarketError(cause, t));
    } finally {
      setBusy(false);
    }
  }
  const billedTime = useBilledTime();
  const billed = (item: OnlineSellerItem) => {
    const line = billedTime(item);
    return line ? <span>{line}</span> : null;
  };
  const income = (item: OnlineSellerItem) =>
    item.incomeUnits != null ? formatWalletUnits(item.incomeUnits, "$", locale) : item.cyclePriceMinor ? "—" : formatUsdMoney(0, locale);
  const editButton = (listing: OnlineListing | undefined) =>
    listing?.state === "listed" ? (
      <Button size="sm" variant="outline" className="h-9" isDisabled={busy} onPress={() => setEditing(listing)}>
        <PencilLine className="size-3.5" aria-hidden />
        {t("clientMarket.edit.action")}
      </Button>
    ) : null;
  const pauseButton = (item: OnlineSellerItem, listing: OnlineListing | undefined) =>
    listing?.state === "listed" && item.listingId ? (
      <Button
        size="sm"
        variant="outline"
        className="h-9"
        isDisabled={busy}
        onPress={() =>
          void action(async () => {
            await setOnlineListingPaused(listing.installationId, !item.paused);
            toast.success(t(item.paused ? "clientMarket.pause.resumed" : "clientMarket.pause.paused"));
          })
        }
      >
        {item.paused ? <Play className="size-3.5" aria-hidden /> : <Pause className="size-3.5" aria-hidden />}
        {t(item.paused ? "clientMarket.pause.resume" : "clientMarket.pause.pause")}
      </Button>
    ) : null;

  const historyColumns: RecordTableColumn<OnlineSellerItem>[] = [
    {
      id: "client",
      header: t("online.clientColumn"),
      cell: (row) => (
        <div className="grid min-w-0 gap-0.5">
          <span className="truncate font-medium" title={nameOf(row)}>{nameOf(row)}</span>
          <span className="truncate text-xs text-muted-foreground md:hidden">{row.tenantEmail || "—"}</span>
        </div>
      ),
    },
    {
      id: "tenant",
      header: t("clientMarket.seller.tenant"),
      minWidth: "md",
      cell: (row) => <span className="block truncate text-muted-foreground" title={row.tenantEmail || undefined}>{row.tenantEmail || "—"}</span>,
    },
    {
      id: "ended",
      header: t("rentalAudit.ended"),
      minWidth: "lg",
      className: "w-40",
      cell: (row) => <span className="text-muted-foreground">{date(row.endedAt)}</span>,
    },
    {
      id: "reason",
      header: t("clientRentals.reason"),
      minWidth: "md",
      className: "w-40",
      cell: (row) => <span className="block truncate">{row.endReason ? t(endReasonKey(row.endReason)) : "—"}</span>,
    },
    {
      id: "income",
      header: t("clientMarket.seller.income"),
      className: "w-24 sm:w-28",
      cell: (row) => <span className="block truncate tabular-nums">{income(row)}</span>,
    },
  ];

  if (!loaded) return <p role="status" className="py-10 text-center text-sm text-muted-foreground">{t("common.loading")}</p>;
  return (
    <div className="grid min-w-0 gap-4">
      {addOpen ? <ClientListingAddDialog initialId={initialInstallation} onClose={onAddClose} onListed={onChanged} /> : null}
      <OnlineSellingToolbar listedCount={listed.length + rented.length} query={query} onQuery={setQuery} onAdd={onAddOpen} />
      {error ? <p role="alert" className="text-sm text-destructive">{error}</p> : null}
      {visibleListed.length === 0 && visibleRented.length === 0 && history.length === 0 ? (
        <p className="rounded-xl border border-dashed border-border px-4 py-10 text-center text-sm text-muted-foreground">
          {t(text ? "clientMarket.shop.sellingSearchEmpty" : "clientMarket.shop.sellingEmpty")}
        </p>
      ) : null}

      {visibleListed.length ? (
        <div className={ONLINE_CLIENT_CARD_GRID_CLASS} aria-label={t("clientMarket.shop.cardGrid")}>
          {visibleListed.map((item) => {
            const listing = item.listingId ? listingById.get(item.listingId) : undefined;
            const transitional = ["locking", "unlocking"].includes(item.state);
            return (
              <OnlineClientCard
                key={item.id}
                cardId={`market-client-${item.listingId || item.id}`}
                name={nameOf(item)}
                price={price(item.cyclePriceMinor)}
                status={t(item.paused ? "online.paused" : onlineStatusKey(item.state))}
                tone={item.paused ? "attention" : undefined}
                detail={item.title ? <span className="font-mono">{item.label}</span> : null}
                listing={listing}
                actions={
                  item.listingId ? (
                    <>
                      {!transitional ? editButton(listing) : null}
                      {!transitional ? pauseButton(item, listing) : null}
                      <Button
                        size="sm"
                        variant="ghost"
                        className="h-9"
                        isDisabled={busy || !listing}
                        onPress={() => listing && setUnlisting(listing)}
                      >
                        {t("online.unlist")}
                      </Button>
                    </>
                  ) : null
                }
                extra={
                  <div className="grid gap-2">
                    {listing?.lastError && listing.observedAt != null && transitional ? (
                      <p role="status" className="text-xs text-muted-foreground">
                        {t("online.probe")} · {date(listing.observedAt)} · {t(probeKey(listing.lastError))}
                        {listing.attempts != null && ` · ${t("metrics.shareControl.attempts", { count: listing.attempts })}`}
                      </p>
                    ) : null}
                    {listing && transitional ? (
                      <Button
                        size="sm"
                        variant="outline"
                        className="h-9 justify-self-start"
                        isDisabled={busy}
                        onPress={() =>
                          void action(async () => {
                            await onlineRequest(`installations/${listing.installationId}/retry`, "POST");
                          })
                        }
                      >
                        <RotateCcw className="size-3.5" aria-hidden />
                        {t("common.retry")}
                      </Button>
                    ) : null}
                    {item.listingId && item.id === item.listingId ? (
                      <details className="group min-w-0">
                        <summary className="flex min-h-9 cursor-pointer items-center text-xs font-medium text-muted-foreground hover:text-foreground">
                          {t("dashboard.clientOnlineCalendar.title")}
                        </summary>
                        <ClientOnlineHeatmap installationId={item.listingId} />
                      </details>
                    ) : null}
                    <ClientTerminationHostAction summary={terminations.get(item.id)} onOpen={onTerminationOpen} />
                  </div>
                }
              />
            );
          })}
        </div>
      ) : null}

      {visibleRented.length ? (
        <section className="grid gap-2 border-t border-slate-200 pt-5" aria-labelledby="client-seller-rented">
          <h3 id="client-seller-rented" className="text-xs font-semibold uppercase tracking-wide text-slate-500">
            {t("clientMarket.shop.filter.rented")}
            <span className="ml-1.5 tabular-nums text-slate-400">{rented.length}</span>
          </h3>
          <div className={ONLINE_CLIENT_CARD_GRID_CLASS}>
            {visibleRented.map((item) => {
              const listing = item.listingId ? listingById.get(item.listingId) : undefined;
              const pending = item.reclaimEffectiveAt != null && ["starting", "active"].includes(item.state);
              // A due reclaim is about to run and can no longer be cancelled.
              const cancellable = pending && item.reclaimEffectiveAt! * 1000 > Date.now();
              return (
                <OnlineClientCard
                  key={item.id}
                  cardId={`market-client-${item.id}`}
                  name={nameOf(item)}
                  price={price(item.cyclePriceMinor)}
                  status={pending ? t("clientMarket.reclaim.scheduledShort", { date: date(item.reclaimEffectiveAt) }) : t(onlineStatusKey(item.state))}
                  tone={pending ? "attention" : "good"}
                  detail={
                    <>
                      <span>{item.tenantEmail ? t("clientMarket.shop.tenantLine", { email: item.tenantEmail }) : t("clientMarket.shop.rentedOut")}</span>
                      {item.expiresAt ? <span>{t("clientMarket.shop.expires", { date: date(item.expiresAt) })}</span> : null}
                      {billed(item)}
                      {item.incomeUnits != null ? <span>{t("clientMarket.seller.incomeLine", { amount: income(item) })}</span> : null}
                    </>
                  }
                  listing={listing}
                  actions={
                    <>
                      {cancellable ? (
                        <Button
                          size="sm"
                          variant="outline"
                          className="h-9"
                          isDisabled={busy}
                          onPress={() =>
                            void action(async () => {
                              const result = await cancelOnlineReclaim(item.id);
                              toast.success(t(result?.renewalRestored ? "clientMarket.reclaim.cancelledRenewal" : "clientMarket.reclaim.cancelled"));
                            })
                          }
                        >
                          <Undo2 className="size-3.5" aria-hidden />
                          {t("clientMarket.reclaim.cancel")}
                        </Button>
                      ) : null}
                      {item.reclaimable && item.reclaimMode !== "immediate" ? (
                        <Button
                          size="sm"
                          variant="outline"
                          className="h-9"
                          isDisabled={busy}
                          onPress={() => setReclaiming({ leaseId: item.id, name: nameOf(item), escalate: pending, starting: item.state === "starting" })}
                        >
                          <ClockAlert className="size-3.5" aria-hidden />
                          {t(pending ? "clientMarket.reclaim.escalate" : "clientMarket.reclaim.action")}
                        </Button>
                      ) : null}
                      {editButton(listing)}
                      {listing && listing.state !== "unlisted" ? (
                        <Button size="sm" variant="ghost" className="h-9" isDisabled={busy} onPress={() => setUnlisting(listing)}>
                          {t("online.unlist")}
                        </Button>
                      ) : null}
                      {item.terminationId ? (
                        <Button size="sm" variant="ghost" className="h-9" onPress={() => onTerminationOpen(item.terminationId!)}>
                          {t("online.exitRecord")}
                        </Button>
                      ) : null}
                    </>
                  }
                  extra={
                    <div className="grid gap-2">
                      {listing?.state === "unlisted" && item.listingId ? <ListingSyncDetail installationId={item.listingId} revision={listing.revision} /> : null}
                      <ClientTerminationHostAction
                        summary={item.listingId ? terminations.get(item.listingId) : undefined}
                        onOpen={onTerminationOpen}
                      />
                    </div>
                  }
                />
              );
            })}
          </div>
        </section>
      ) : null}

      {history.length ? (
        <RentalHistorySection id="client-seller-history" title={t("lease.history")} count={history.length}>
          <ExpandableRecordTable
            rows={pagedHistory}
            rowKey={(row) => row.id}
            rowLabel={nameOf}
            columns={historyColumns}
            ariaLabel={t("lease.history")}
            renderDetail={(row) => (
              <div className="grid gap-3">
                <RecordDetailList
                  items={[
                    { label: t("online.clientColumn"), value: <span className="break-all">{row.label}</span> },
                    { label: t("clientMarket.seller.tenant"), value: row.tenantEmail || "—" },
                    { label: t("rentalAudit.price"), value: price(row.cyclePriceMinor) },
                    { label: t("rentalAudit.activated"), value: date(row.startedAt) },
                    { label: t("rentalAudit.ended"), value: date(row.endedAt) },
                    { label: t("clientRentals.reason"), value: row.endReason ? t(endReasonKey(row.endReason)) : "—" },
                    { label: t("clientMarket.seller.income"), value: income(row) },
                  ]}
                />
                {row.listingId ? <ListingSyncDetail installationId={row.listingId} revision={listingById.get(row.listingId)?.revision} /> : null}
                {row.terminationId ? (
                  <Button size="sm" variant="outline" className="justify-self-start" onPress={() => onTerminationOpen(row.terminationId!)}>
                    {t("online.exitRecord")}
                  </Button>
                ) : null}
              </div>
            )}
          />
          <MarketPagination page={shownHistoryPage} pageCount={historyPages} onPageChange={setHistoryPage} />
        </RentalHistorySection>
      ) : null}

      <Modal.Backdrop isOpen={!!unlisting} onOpenChange={(open) => !open && !busy && setUnlisting(null)}>
        <Modal.Container placement="center">
          <Modal.Dialog className="w-[min(420px,calc(100vw-2rem))]">
            <Modal.Header>
              <Modal.Heading>
                {t("online.unlist")} · {unlisting ? clientDisplayName(unlisting) : ""}
              </Modal.Heading>
            </Modal.Header>
            <Modal.Body className="grid gap-2 text-sm">
              <p>{t("online.unlistNotice")}</p>
              <p className="text-xs text-muted-foreground">{t("clientMarket.pause.instead")}</p>
              {error ? <p role="alert" className="text-destructive">{error}</p> : null}
            </Modal.Body>
            <Modal.Footer>
              <Button variant="ghost" isDisabled={busy} onPress={() => setUnlisting(null)}>
                {t("common.cancel")}
              </Button>
              <Button
                variant="danger"
                isDisabled={busy}
                onPress={() =>
                  void action(async () => {
                    if (!unlisting) return;
                    await onlineRequest(`installations/${encodeURIComponent(unlisting.installationId)}/listing`, "DELETE");
                    toast.success(t("online.unlisted"));
                    setUnlisting(null);
                  })
                }
              >
                {t("online.unlist")}
              </Button>
            </Modal.Footer>
          </Modal.Dialog>
        </Modal.Container>
      </Modal.Backdrop>
      <ClientListingEditDialog listing={editing} onClose={() => setEditing(null)} onSaved={onChanged} />
      <ClientReclaimDialog
        target={reclaiming}
        onClose={() => setReclaiming(null)}
        onDone={() => {
          toast.success(t("clientMarket.reclaim.done"));
          onChanged();
        }}
      />
    </div>
  );
}
