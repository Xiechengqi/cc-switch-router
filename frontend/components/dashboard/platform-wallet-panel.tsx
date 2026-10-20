"use client";

/* Hallmark · component: platform wallet panel · genre: modern-minimal · theme: existing Router system
 * states: default · hover · focus · active · disabled · loading · error · success
 * contrast: inherited Router tokens · pre-emit critique: P4 H5 E4 S5 R5 V4
 */

import * as React from "react";
import Link from "next/link";
import { useSearchParams } from "next/navigation";
import { Button } from "@heroui/react";
import { ArrowUpRight, Ban, CheckCircle2, Clock3, CreditCard, Loader2, RefreshCw, WalletCards } from "lucide-react";
import { ConfirmAlertDialog } from "@/components/common/confirm-alert-dialog";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { MarketWalletRefundPanel } from "@/components/dashboard/market-wallet-refund-panel";
import { ApiError } from "@/lib/api";
import { DASHBOARD_ACCOUNT_BILLING_PATH } from "@/lib/dashboard-nav";
import {
  createMarketWalletTopup,
  cancelMarketWalletTopup,
  getMarketWallet,
  getMarketWalletChannels,
  getMarketWalletTopup,
  getMarketWalletTopupGatewayStatus,
  getMarketWalletTopups,
  getMarketWalletTransactions,
  type MarketWalletChannel,
  type MarketWallet,
  type MarketWalletTopup,
  type MarketTopupGatewayStatus,
  type MarketWalletTransaction,
} from "@/lib/market-wallet-api";
import { formatWalletUnits, formatWalletUnitsAsCny, parseUsdMinor, payableCheckoutUrl, safeMarketReturnPath, topupDisplayStatus } from "@/lib/market-wallet-money";

const FINAL_TOPUP_STATUS = new Set(["credited", "review_required", "expired", "cancelled"]);

function errorText(cause: unknown) {
  return cause instanceof Error ? cause.message : String(cause);
}

function dateText(value: string, locale: string) {
  const date = new Date(value);
  return Number.isNaN(date.getTime())
    ? value
    : new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short" }).format(date);
}

export function WalletActionButton({
  label,
  phase = "default",
  disabled = false,
  previewState,
}: {
  label: string;
  phase?: "default" | "loading" | "error" | "success";
  disabled?: boolean;
  previewState?: "hover" | "focus" | "active";
}) {
  return (
    <button
      type="submit"
      disabled={disabled || phase === "loading"}
      data-phase={phase}
      data-preview={previewState}
      className="inline-flex h-11 min-w-0 items-center justify-center gap-2 whitespace-nowrap rounded-md bg-primary px-4 text-sm font-semibold text-primary-foreground transition-transform duration-100 hover:bg-primary/90 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring active:translate-y-px disabled:cursor-not-allowed disabled:opacity-50 data-[phase=error]:bg-destructive data-[preview=hover]:bg-primary/90 data-[preview=focus]:outline-2 data-[preview=focus]:outline-offset-2 data-[preview=focus]:outline-ring data-[preview=active]:translate-y-px motion-reduce:transition-none"
    >
      {phase === "loading" ? <Loader2 className="size-4 animate-spin motion-reduce:animate-none" aria-hidden="true" />
        : phase === "success" ? <CheckCircle2 className="size-4" aria-hidden="true" />
          : <CreditCard className="size-4" aria-hidden="true" />}
      {label}
    </button>
  );
}

export function PlatformWalletPanel({ initialWallet }: { initialWallet: MarketWallet }) {
  const { locale, t, displayCurrency, fxRateFresh, fxRateMicros } = useLocaleText();
  const searchParams = useSearchParams();
  const returnedTopupId = searchParams.get("topup");
  const [returnPath, setReturnPath] = React.useState<string | null>(null);
  const [wallet, setWallet] = React.useState(initialWallet);
  const [channels, setChannels] = React.useState<MarketWalletChannel[]>([]);
  const [topups, setTopups] = React.useState<MarketWalletTopup[]>([]);
  const [gatewayStatuses, setGatewayStatuses] = React.useState<Record<string, MarketTopupGatewayStatus | null>>({});
  const [statusBusyId, setStatusBusyId] = React.useState<string | null>(null);
  const [cancelConfirmId, setCancelConfirmId] = React.useState<string | null>(null);
  const [transactions, setTransactions] = React.useState<MarketWalletTransaction[]>([]);
  const [topupCursor, setTopupCursor] = React.useState<string | null>(null);
  const [transactionCursor, setTransactionCursor] = React.useState<string | null>(null);
  const [returnedTopup, setReturnedTopup] = React.useState<MarketWalletTopup | null>(null);
  const [channelId, setChannelId] = React.useState("");
  const [amount, setAmount] = React.useState("");
  const [amountTouched, setAmountTouched] = React.useState(false);
  const [busy, setBusy] = React.useState("");
  const [error, setError] = React.useState("");
  const [loaded, setLoaded] = React.useState(false);
  const idempotencyRef = React.useRef<{ fingerprint: string; key: string } | null>(null);
  const busyRef = React.useRef(false);
  const statusRequestVersionRef = React.useRef<Record<string, number>>({});

  React.useEffect(() => {
    const requested = safeMarketReturnPath(searchParams.get("returnTo"));
    let saved: string | null = null;
    try {
      if (returnedTopupId) {
        saved = safeMarketReturnPath(window.sessionStorage.getItem(`marketWalletReturnTo:${returnedTopupId}`));
      }
    } catch {
      // Storage is optional; the wallet and checkout remain usable without it.
    }
    setReturnPath(requested ?? saved);
    const suggested = searchParams.get("amount");
    if (suggested && parseUsdMinor(suggested) != null) {
      setAmount((current) => current || suggested);
    }
  }, [returnedTopupId, searchParams]);

  React.useEffect(() => {
    const pendingIds = topups.filter((order) => order.status === "pending").map((order) => order.id);
    if (!pendingIds.length) return;
    let active = true;
    const requests = pendingIds.map((id) => {
      const version = (statusRequestVersionRef.current[id] ?? 0) + 1;
      statusRequestVersionRef.current[id] = version;
      return { id, version };
    });
    void Promise.all(requests.map(async ({ id, version }) => {
      try {
        return { id, version, status: await getMarketWalletTopupGatewayStatus(id) };
      } catch {
        return { id, version, status: null };
      }
    })).then((results) => {
      if (!active) return;
      setGatewayStatuses((current) => ({
        ...current,
        ...Object.fromEntries(results
          .filter(({ id, version }) => statusRequestVersionRef.current[id] === version)
          .map(({ id, status }) => [id, status])),
      }));
    });
    return () => { active = false; };
  }, [topups]);

  const reload = React.useCallback(async () => {
    const [nextWallet, nextChannels, nextTopups, nextTransactions] = await Promise.all([
      getMarketWallet(), getMarketWalletChannels(),
      getMarketWalletTopups(), getMarketWalletTransactions(),
    ]);
    setWallet(nextWallet);
    setChannels(nextChannels);
    setGatewayStatuses({});
    setTopups(nextTopups.items);
    setTopupCursor(nextTopups.nextCursor);
    setTransactions(nextTransactions.items);
    setTransactionCursor(nextTransactions.nextCursor);
    setChannelId((current) => current || nextChannels[0]?.id || "");
    setLoaded(true);
  }, []);

  React.useEffect(() => {
    let active = true;
    Promise.all([
      getMarketWallet(), getMarketWalletChannels(),
      getMarketWalletTopups(), getMarketWalletTransactions(),
    ]).then(([nextWallet, nextChannels, nextTopups, nextTransactions]) => {
      if (!active) return;
      setWallet(nextWallet);
      setChannels(nextChannels);
      setTopups(nextTopups.items);
      setTopupCursor(nextTopups.nextCursor);
      setTransactions(nextTransactions.items);
      setTransactionCursor(nextTransactions.nextCursor);
      setChannelId((current) => current || nextChannels[0]?.id || "");
      setLoaded(true);
    }).catch((cause) => {
      if (!active) return;
      setError(errorText(cause));
      setLoaded(true);
    });
    return () => { active = false; };
  }, []);

  React.useEffect(() => {
    if (!returnedTopupId) return;
    let active = true;
    let timeout: ReturnType<typeof setTimeout> | undefined;
    const retry = () => {
      if (active) timeout = setTimeout(poll, 5_000);
    };
    const poll = async () => {
      let credited = false;
      try {
        const current = await getMarketWalletTopup(returnedTopupId);
        if (!active) return;
        setReturnedTopup(current);
        credited = current.status === "credited";
        if (credited) await reload();
        setError("");
        if (!FINAL_TOPUP_STATUS.has(current.status)) retry();
      } catch (cause) {
        if (!active) return;
        setError(errorText(cause));
        // A temporary API failure (including one during the balance refresh)
        // must not strand a paid order on a stale balance. A rejected order ID
        // is terminal and should not spin forever.
        if (credited || !(cause instanceof ApiError) || cause.status >= 500 || cause.status === 429) retry();
      }
    };
    void poll();
    return () => { active = false; if (timeout) clearTimeout(timeout); };
  }, [returnedTopupId, reload]);

  const parsedAmount = parseUsdMinor(amount);
  const amountError = amountTouched && amount && parsedAmount == null;
  const submitTopup = async (event: React.FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    setAmountTouched(true);
    if (busyRef.current || !wallet.canTopup || !channelId || parsedAmount == null) return;
    busyRef.current = true;
    setBusy("topup");
    setError("");
    const fingerprint = `${channelId}:${parsedAmount}`;
    if (idempotencyRef.current?.fingerprint !== fingerprint) {
      idempotencyRef.current = { fingerprint, key: crypto.randomUUID() };
    }
    try {
      const order = await createMarketWalletTopup(channelId, parsedAmount, idempotencyRef.current.key);
      setTopups((current) => [order, ...current.filter((item) => item.id !== order.id)]);
      if (returnPath) {
        try {
          window.sessionStorage.setItem(`marketWalletReturnTo:${order.id}`, returnPath);
        } catch {
          // A blocked storage API must not block payment.
        }
      }
      const checkoutUrl = payableCheckoutUrl(order.status, order.checkoutUrl);
      if (checkoutUrl) {
        window.location.assign(checkoutUrl);
      } else if (order.status !== "credited") {
        setError(t(order.status === "review_required" ? "marketWallet.reviewHelp" : "marketWallet.checkoutPending"));
      }
    } catch (cause) {
      setError(cause instanceof ApiError && cause.code === "MARKET_TOPUP_PENDING_LIMIT"
        ? t("marketWallet.pendingLimit")
        : cause instanceof ApiError && cause.code === "MARKET_TOPUP_CHECKOUT_UNAVAILABLE"
          ? t("marketWallet.checkoutUnavailable")
          : errorText(cause));
      void reload().catch(() => {});
    } finally {
      busyRef.current = false;
      setBusy("");
    }
  };

  const refreshGatewayStatus = async (id: string) => {
    if (statusBusyId) return;
    const version = (statusRequestVersionRef.current[id] ?? 0) + 1;
    statusRequestVersionRef.current[id] = version;
    setStatusBusyId(id);
    try {
      const status = await getMarketWalletTopupGatewayStatus(id);
      if (statusRequestVersionRef.current[id] === version) {
        setGatewayStatuses((current) => ({ ...current, [id]: status }));
      }
      setError("");
    } catch {
      if (statusRequestVersionRef.current[id] === version) {
        setGatewayStatuses((current) => ({ ...current, [id]: null }));
      }
      setError(t("marketWallet.gatewayStatusUnavailable"));
    } finally {
      setStatusBusyId(null);
    }
  };

  const cancelSelectedTopup = async () => {
    const id = cancelConfirmId;
    if (!id || busyRef.current) return;
    busyRef.current = true;
    setBusy("cancelTopup");
    setError("");
    try {
      const updated = await cancelMarketWalletTopup(id);
      setTopups((current) => current.map((order) => order.id === id ? updated : order));
      setCancelConfirmId(null);
      try {
        await reload();
      } catch {
        setError(t("marketWallet.cancelRefreshFailed"));
      }
    } catch (cause) {
      setCancelConfirmId(null);
      setError(cause instanceof ApiError && cause.status === 409
        ? t("marketWallet.cancelChanged")
        : errorText(cause));
      await reload().catch(() => {});
    } finally {
      busyRef.current = false;
      setBusy("");
    }
  };

  const loadMore = async (kind: "topups" | "transactions") => {
    if (busyRef.current) return;
    const cursor = kind === "topups" ? topupCursor : transactionCursor;
    if (!cursor) return;
    busyRef.current = true;
    setBusy(kind);
    setError("");
    try {
      if (kind === "topups") {
        const page = await getMarketWalletTopups(cursor);
        setTopups((current) => [...current, ...page.items]);
        setTopupCursor(page.nextCursor);
      } else {
        const page = await getMarketWalletTransactions(cursor);
        setTransactions((current) => [...current, ...page.items]);
        setTransactionCursor(page.nextCursor);
      }
    } catch (cause) {
      setError(errorText(cause));
    } finally {
      busyRef.current = false;
      setBusy("");
    }
  };

  return (
    <main className="mx-auto grid w-full max-w-5xl min-w-0 gap-6 px-4 py-6 sm:px-6" aria-label={t("marketWallet.title")}>
      <header className="flex min-w-0 flex-wrap items-start justify-between gap-3">
        <div className="min-w-0">
          <h1 className="break-words text-2xl font-semibold tracking-tight">{t("marketWallet.title")}</h1>
          <p className="mt-1 text-sm text-muted-foreground">{t("marketWallet.subtitle")}</p>
        </div>
        <Button variant="outline" size="sm" isDisabled={!!busy} onPress={() => void reload().catch((cause) => setError(errorText(cause)))}>
          <RefreshCw className="size-4" aria-hidden="true" />{t("common.reload")}
        </Button>
      </header>

      {error ? <p role="alert" className="rounded-md border border-destructive/30 bg-destructive/5 px-4 py-3 text-sm text-destructive">{error}</p> : null}
      {wallet.mode === "shadow" ? <p className="rounded-md border border-amber-300 bg-amber-50 px-4 py-3 text-sm text-amber-900">{t("marketWallet.shadow")}</p> : null}
      {returnedTopup ? (
        <div role="status" aria-live="polite" className="flex min-w-0 flex-wrap items-center justify-between gap-3 rounded-md border border-border bg-card px-4 py-3 text-sm">
          <span className="flex items-center gap-2">
            {returnedTopup.status === "credited" ? <CheckCircle2 className="size-4 text-emerald-700" aria-hidden="true" /> : <Clock3 className="size-4 text-muted-foreground" aria-hidden="true" />}
            {t(`marketWallet.status.${returnedTopup.status}` as Parameters<typeof t>[0])}
          </span>
          {returnedTopup.status === "credited" && returnPath ? (
            <Link href={returnPath} className="inline-flex min-h-11 items-center whitespace-nowrap font-medium text-primary underline-offset-2 hover:underline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring">
              {t("marketWallet.returnToMarket")}<ArrowUpRight className="ml-1 size-4" aria-hidden="true" />
            </Link>
          ) : null}
        </div>
      ) : null}

      <section className="grid min-w-0 gap-5 rounded-xl border border-border bg-card p-5 sm:grid-cols-[minmax(0,1fr)_minmax(0,1fr)] sm:p-6">
        <div className="min-w-0">
          <p className="flex items-center gap-2 text-sm text-muted-foreground"><WalletCards className="size-4" aria-hidden="true" />{t("marketWallet.available")}</p>
          <p className="mt-2 break-all text-3xl font-semibold tabular-nums tracking-tight sm:text-4xl">{displayCurrency === "CNY" && fxRateFresh ? formatWalletUnitsAsCny(wallet.availableUnits, fxRateMicros, locale) : formatWalletUnits(wallet.availableUnits)}</p>
          {displayCurrency === "CNY" && fxRateFresh ? <p className="mt-1 text-xs text-muted-foreground">{t("marketWallet.displayOnly", { amount: formatWalletUnits(wallet.availableUnits) })}</p> : null}
          <p className="mt-2 text-sm text-muted-foreground">{t("marketWallet.held")}: {displayCurrency === "CNY" && fxRateFresh ? formatWalletUnitsAsCny(wallet.heldUnits, fxRateMicros, locale) : formatWalletUnits(wallet.heldUnits)}</p>
        </div>
        {wallet.canTopup ? <form className="grid min-w-0 content-start gap-3" onSubmit={(event) => void submitTopup(event)}>
          <p className="text-sm font-semibold">{t("marketWallet.topup")}</p>
          <label className="grid min-w-0 gap-1 text-sm">
            <span>{t("marketWallet.channel")}</span>
            <select
              className="h-11 min-w-0 rounded-md border border-input bg-background px-3 outline-2 outline-transparent focus-visible:outline-ring disabled:cursor-not-allowed disabled:opacity-50"
              value={channelId} onChange={(event) => { setChannelId(event.target.value); idempotencyRef.current = null; }}
              disabled={!wallet.canTopup || !channels.length || !!busy}
            >
              {!channels.length ? <option value="">{t("marketWallet.noChannel")}</option> : null}
              {channels.map((channel) => <option key={channel.id} value={channel.id}>{channel.displayName}</option>)}
            </select>
          </label>
          <label className="grid min-w-0 gap-1 text-sm">
            <span>{t("marketWallet.amount")}</span>
            <input
              inputMode="decimal" autoComplete="off" value={amount}
              onChange={(event) => { setAmount(event.target.value); idempotencyRef.current = null; }}
              onBlur={() => setAmountTouched(true)}
              aria-invalid={!!amountError} aria-describedby="wallet-amount-help"
              className="h-11 min-w-0 rounded-md border border-input bg-background px-3 outline-2 outline-transparent focus-visible:outline-ring aria-invalid:border-destructive disabled:cursor-not-allowed disabled:opacity-50"
              placeholder="10.00" disabled={!wallet.canTopup || !!busy}
            />
            <span id="wallet-amount-help" className={`min-h-[1lh] text-xs ${amountError ? "text-destructive" : "text-muted-foreground"}`}>
              {amountError ? t("marketWallet.amountError") : t("marketWallet.amountHelp")}
            </span>
          </label>
          <WalletActionButton
            label={busy === "topup" ? t("marketWallet.creating") : t("marketWallet.pay")}
            phase={busy === "topup" ? "loading" : error ? "error" : "default"}
            disabled={!wallet.canTopup || !channels.length || parsedAmount == null || !!busy}
          />
          {!channels.length ? <p className="text-xs text-muted-foreground">{t("marketWallet.noChannelHelp")}</p> : null}
        </form> : <div className="grid content-start gap-3 rounded-lg bg-muted/50 p-4 text-sm text-muted-foreground">
          <p>{t(wallet.mode === "enabled" ? "marketWallet.topupUnavailable" : "marketWallet.notOpen")}</p>
          {wallet.mode !== "enabled" ? <Link href={DASHBOARD_ACCOUNT_BILLING_PATH} className="font-medium text-foreground underline underline-offset-2">{t("marketWallet.legacyBilling")}</Link> : null}
        </div>}
      </section>

      {(wallet.canTopup || wallet.hasWalletActivity) && !loaded ? <p className="text-sm text-muted-foreground">{t("common.loading")}</p> : null}
      {wallet.canTopup || wallet.hasWalletActivity ? <div className="grid min-w-0 gap-6 lg:grid-cols-2">
        <section className="min-w-0 rounded-xl border border-border bg-card p-5" aria-label={t("marketWallet.topups")}>
          <h2 className="text-base font-semibold">{t("marketWallet.topups")}</h2>
          <ul className="mt-3 grid gap-2">
            {topups.map((order) => {
              const gateway = gatewayStatuses[order.id];
              const shownStatus = order.status === "pending" && gateway === undefined
                ? "gateway_checking"
                : order.status === "pending" && gateway === null
                  ? "gateway_unavailable"
                  : topupDisplayStatus(order.status, gateway?.gatewayStatus);
              const deadline = gateway?.gatewayStatus === "awaiting_payment"
                ? gateway.selectionDeadline
                : gateway?.expiresAt;
              const checkoutUrl = payableCheckoutUrl(order.status, order.checkoutUrl, gateway?.gatewayStatus ?? null);
              return <li key={order.id} className="flex min-w-0 flex-wrap items-center justify-between gap-2 rounded-md bg-muted/50 px-3 py-2 text-sm">
                <div className="min-w-0">
                  <p className="font-medium tabular-nums">${(order.amountMinor / 100).toFixed(2)}</p>
                  <p className="text-xs text-muted-foreground">{dateText(order.createdAt, locale)}</p>
                  {deadline && order.status === "pending" ? <p className="text-xs text-muted-foreground">{t("marketWallet.gatewayDeadline", { time: dateText(deadline, locale) })}</p> : null}
                  {gateway?.gatewayStatus === "awaiting_payment" && order.status === "pending" ? <p className="text-xs text-muted-foreground">{t("marketWallet.awaitingMethodHelp")}</p> : null}
                  {order.status === "review_required" ? <p className="mt-1 text-xs text-amber-700">{t("marketWallet.reviewHelp")}</p> : null}
                </div>
                <div className="flex flex-wrap items-center gap-2">
                  <span className="text-xs text-muted-foreground">{t(`marketWallet.status.${shownStatus}` as Parameters<typeof t>[0])}</span>
                  {order.status === "pending" ? <Button size="sm" variant="ghost" isIconOnly aria-label={t("marketWallet.refreshGatewayStatus")} isDisabled={statusBusyId === order.id || !!busy} onPress={() => void refreshGatewayStatus(order.id)}>
                    <RefreshCw className={`size-4 ${statusBusyId === order.id ? "animate-spin" : ""}`} aria-hidden="true" />
                  </Button> : null}
                  {gateway?.canCancel && order.status === "pending" ? <Button size="sm" variant="outline" isDisabled={!!busy || statusBusyId === order.id} onPress={() => setCancelConfirmId(order.id)}>
                    <Ban className="size-4" aria-hidden="true" />{t("marketWallet.cancelPayment")}
                  </Button> : null}
                  {checkoutUrl ? <a className="inline-flex min-h-11 items-center gap-1 whitespace-nowrap text-xs font-medium text-primary underline-offset-2 hover:underline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring" href={checkoutUrl}>
                    {t("marketWallet.continuePay")}<ArrowUpRight className="size-3" aria-hidden="true" />
                  </a> : null}
                </div>
              </li>;
            })}
            {loaded && !topups.length ? <li className="text-sm text-muted-foreground">{t("marketWallet.noTopups")}</li> : null}
          </ul>
          {topupCursor ? <Button size="sm" variant="ghost" isDisabled={!!busy} onPress={() => void loadMore("topups")}>{t("marketWallet.more")}</Button> : null}
        </section>
        <section className="min-w-0 rounded-xl border border-border bg-card p-5" aria-label={t("marketWallet.transactions")}>
          <h2 className="text-base font-semibold">{t("marketWallet.transactions")}</h2>
          <ul className="mt-3 grid gap-2">
            {transactions.map((entry) => (
              <li key={entry.id} className="flex min-w-0 items-center justify-between gap-2 rounded-md bg-muted/50 px-3 py-2 text-sm">
                <div className="min-w-0">
                  <p className="truncate font-medium">{t(`marketWallet.entry.${entry.sourceKind}` as Parameters<typeof t>[0])}</p>
                  <p className="text-xs text-muted-foreground">{dateText(entry.createdAt, locale)}</p>
                </div>
                <span className="shrink-0 tabular-nums">{formatWalletUnits(entry.deltaUnits)}</span>
              </li>
            ))}
            {loaded && !transactions.length ? <li className="text-sm text-muted-foreground">{t("marketWallet.noTransactions")}</li> : null}
          </ul>
          {transactionCursor ? <Button size="sm" variant="ghost" isDisabled={!!busy} onPress={() => void loadMore("transactions")}>{t("marketWallet.more")}</Button> : null}
        </section>
      </div> : null}
      {wallet.hasWalletActivity ? <MarketWalletRefundPanel availableUnits={wallet.availableUnits} onSubmitted={reload} /> : null}
      <ConfirmAlertDialog
        open={!!cancelConfirmId}
        title={t("marketWallet.cancelPaymentTitle")}
        description={t("marketWallet.cancelPaymentNote")}
        confirmLabel={t("marketWallet.cancelPayment")}
        cancelLabel={t("common.cancel")}
        tone="warning"
        busy={busy === "cancelTopup"}
        onConfirm={() => void cancelSelectedTopup()}
        onOpenChange={(open) => { if (busy !== "cancelTopup" && !open) setCancelConfirmId(null); }}
      />
    </main>
  );
}
