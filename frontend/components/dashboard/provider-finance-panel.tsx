"use client";

/* Provider finance: balances, earnings lots, active holds, withdrawals (with
 * self-cancel) and a two-step, email-confirmed withdrawal address change.
 */

import * as React from "react";
import { Button } from "@heroui/react";
import { Loader2, MailCheck, ShieldAlert } from "lucide-react";
import { ConfirmAlertDialog } from "@/components/common/confirm-alert-dialog";
import { useLocaleText } from "@/components/i18n/locale-provider";
import {
  CopyTextButton,
  TronTransactionLink,
  WALLET_INPUT,
  WalletBadge,
  WalletSectionLabel,
  type WalletBadgeTone,
} from "@/components/market/wallet-primitives";
import { ApiError } from "@/lib/api";
import { messages, type MessageKey } from "@/lib/i18n";
import {
  cancelMarketProviderWithdrawal,
  getMarketProviderEarnings,
  getMarketProviderFinance,
  getMarketProviderHolds,
  getMarketProviderWithdrawals,
  requestMarketProviderDestinationChallenge,
  requestMarketProviderWithdrawal,
  saveMarketProviderDestination,
  type MarketEarningHold,
  type MarketProviderEarningLot,
  type MarketProviderFinance,
  type MarketProviderWithdrawal,
} from "@/lib/market-wallet-api";
import { formatWalletUnits, parseWithdrawalMinor } from "@/lib/market-wallet-money";

const UNITS_PER_MINOR = 86_400n;
const MAX_WITHDRAWAL_MINOR = 500_000;
const MAX_OPEN_WITHDRAWALS = 3;
const OPEN_WITHDRAWAL_STATUSES = new Set(["requested", "approved", "transfer_in_progress", "transfer_failed", "review_required"]);
const TRON_ADDRESS = /^T[1-9A-HJ-NP-Za-km-z]{33}$/;

type AddressChallenge = { address: string; challengeId: string; expiresAtMs: number };

function errorText(cause: unknown) {
  return cause instanceof Error ? cause.message : String(cause);
}

function units(raw: string) {
  try { return BigInt(raw); } catch { return 0n; }
}

function holdReasonKey(kind: string): MessageKey {
  const key = `marketProviderFinance.holdReason.${kind}`;
  return (key in messages.en ? key : "marketProviderFinance.holdReason.other") as MessageKey;
}

function lotState(lot: MarketProviderEarningLot, nowMs: number): { key: MessageKey; tone: WalletBadgeTone } {
  if (lot.disputed) return { key: "marketProviderFinance.lot.disputed", tone: "warning" };
  if (units(lot.heldUnits) > 0n) return { key: "marketProviderFinance.lot.held", tone: "warning" };
  const net = units(lot.amountUnits) - units(lot.reversedUnits);
  if (net <= 0n) return { key: "marketProviderFinance.lot.reversed", tone: "neutral" };
  if (units(lot.paidUnits) >= net) return { key: "marketProviderFinance.lot.paid", tone: "success" };
  if (units(lot.unlockedUnits) > 0n || Date.parse(lot.unlockAt) <= nowMs) return { key: "marketProviderFinance.lot.available", tone: "accent" };
  return { key: "marketProviderFinance.lot.pending", tone: "neutral" };
}

export function ProviderFinancePanel({ walletMode }: { walletMode: "disabled" | "shadow" | "enabled" }) {
  const { locale, t } = useLocaleText();
  const [finance, setFinance] = React.useState<MarketProviderFinance | null>(null);
  const [withdrawals, setWithdrawals] = React.useState<MarketProviderWithdrawal[]>([]);
  const [nextCursor, setNextCursor] = React.useState<string | null>(null);
  const [holds, setHolds] = React.useState<MarketEarningHold[]>([]);
  const [lots, setLots] = React.useState<MarketProviderEarningLot[]>([]);
  const [lotsCursor, setLotsCursor] = React.useState<string | null>(null);
  const [address, setAddress] = React.useState("");
  const [confirmAddress, setConfirmAddress] = React.useState(false);
  const [challenge, setChallenge] = React.useState<AddressChallenge | null>(null);
  const [code, setCode] = React.useState("");
  const [amount, setAmount] = React.useState("");
  const [cancelTarget, setCancelTarget] = React.useState<MarketProviderWithdrawal | null>(null);
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState("");
  const [notice, setNotice] = React.useState("");
  const [nowMs, setNowMs] = React.useState(() => Date.now());
  const idempotencyRef = React.useRef<{ amountMinor: number; key: string } | null>(null);
  const codeInputRef = React.useRef<HTMLInputElement>(null);

  const reload = React.useCallback(async () => {
    const [nextFinance, page, nextHolds, nextLots] = await Promise.all([
      getMarketProviderFinance(), getMarketProviderWithdrawals(),
      getMarketProviderHolds().catch(() => [] as MarketEarningHold[]),
      getMarketProviderEarnings().catch(() => ({ items: [] as MarketProviderEarningLot[], nextCursor: null })),
    ]);
    setFinance(nextFinance);
    setWithdrawals(page.items);
    setNextCursor(page.nextCursor);
    setHolds(nextHolds);
    setLots(nextLots.items);
    setLotsCursor(nextLots.nextCursor);
  }, []);

  React.useEffect(() => {
    let active = true;
    getMarketProviderFinance()
      .then(async (nextFinance) => {
        if (!active) return;
        setFinance(nextFinance);
        if (!nextFinance.isProvider && !nextFinance.hasFinanceActivity) return;
        const [page, nextHolds, nextLots] = await Promise.all([
          getMarketProviderWithdrawals(),
          getMarketProviderHolds().catch(() => [] as MarketEarningHold[]),
          getMarketProviderEarnings().catch(() => ({ items: [] as MarketProviderEarningLot[], nextCursor: null })),
        ]);
        if (!active) return;
        setWithdrawals(page.items);
        setNextCursor(page.nextCursor);
        setHolds(nextHolds);
        setLots(nextLots.items);
        setLotsCursor(nextLots.nextCursor);
      })
      .catch((cause) => { if (active) setError(errorText(cause)); });
    return () => { active = false; };
  }, []);

  React.useEffect(() => {
    if (!challenge) return;
    const timer = window.setInterval(() => setNowMs(Date.now()), 1_000);
    return () => window.clearInterval(timer);
  }, [challenge]);

  React.useEffect(() => {
    if (challenge) codeInputRef.current?.focus();
  }, [challenge]);

  if (!finance) return error ? <p role="alert" className="mx-auto max-w-5xl px-4 text-sm text-destructive">{error}</p> : null;
  if (!finance.isProvider && !finance.hasFinanceActivity) return <p className="p-5 text-sm text-muted-foreground">{t("marketWallet.noTransactions")}</p>;

  const dateText = (value: string) => {
    const date = new Date(value);
    return Number.isNaN(date.getTime()) ? value : new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short" }).format(date);
  };
  const destination = finance.destination;
  const destinationReadyAt = destination ? new Date(destination.usableAt).getTime() : Number.NaN;
  const addressReady = destination && Number.isFinite(destinationReadyAt) && destinationReadyAt <= Date.now();
  const amountMinor = parseWithdrawalMinor(amount);
  const openWithdrawals = withdrawals.filter((item) => OPEN_WITHDRAWAL_STATUSES.has(item.status)).length;
  const tooManyOpen = openWithdrawals >= MAX_OPEN_WITHDRAWALS;
  const amountOverMax = amountMinor != null && amountMinor > MAX_WITHDRAWAL_MINOR;
  const amountReady = amountMinor != null && !amountOverMax
    && BigInt(amountMinor) * UNITS_PER_MINOR <= BigInt(finance.summary.withdrawableUnits);
  const addressValid = TRON_ADDRESS.test(address.trim());
  const challengeRemaining = challenge ? Math.max(0, Math.ceil((challenge.expiresAtMs - nowMs) / 1000)) : 0;
  const codeValid = /^\d{6}$/.test(code.trim());
  const totalHeld = holds.reduce((sum, hold) => sum + units(hold.heldUnits), 0n);

  const sendCode = async (event?: React.FormEvent<HTMLFormElement>) => {
    event?.preventDefault();
    const target = challenge?.address ?? address.trim();
    if (busy || !finance.canWithdraw || !TRON_ADDRESS.test(target) || (!challenge && !confirmAddress)) return;
    setBusy(true);
    setError("");
    setNotice("");
    try {
      const result = await requestMarketProviderDestinationChallenge(target);
      setChallenge({ address: target, challengeId: result.challengeId, expiresAtMs: Date.now() + result.expiresInSeconds * 1000 });
      setNowMs(Date.now());
      setCode("");
      setNotice(t("marketProviderFinance.codeSent"));
    } catch (cause) {
      setError(cause instanceof ApiError && cause.status === 429 ? t("marketProviderFinance.codeRateLimited") : errorText(cause));
    } finally {
      setBusy(false);
    }
  };

  const confirmCode = async (event: React.FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (busy || !finance.canWithdraw || !challenge || !codeValid || challengeRemaining <= 0) return;
    setBusy(true);
    setError("");
    setNotice("");
    try {
      await saveMarketProviderDestination(challenge.address, challenge.challengeId, code.trim());
      await reload();
      setAddress("");
      setConfirmAddress(false);
      setChallenge(null);
      setCode("");
      setNotice(t("marketProviderFinance.addressSaved"));
    } catch (cause) {
      setError(cause instanceof ApiError && (cause.status === 400 || cause.status === 403 || cause.status === 409)
        ? t("marketProviderFinance.codeRejected")
        : errorText(cause));
    } finally {
      setBusy(false);
    }
  };

  const resetChallenge = () => {
    setChallenge(null);
    setCode("");
    setError("");
    setNotice("");
  };

  const requestWithdrawal = async (event: React.FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (busy || !finance.canWithdraw || walletMode !== "enabled" || !addressReady || !amountReady || tooManyOpen || amountMinor == null) return;
    setBusy(true);
    setError("");
    setNotice("");
    if (idempotencyRef.current?.amountMinor !== amountMinor) {
      idempotencyRef.current = { amountMinor, key: crypto.randomUUID() };
    }
    try {
      await requestMarketProviderWithdrawal(amountMinor, idempotencyRef.current.key);
      await reload();
      setAmount("");
      idempotencyRef.current = null;
      setNotice(t("marketProviderFinance.requested"));
    } catch (cause) {
      setError(errorText(cause));
    } finally {
      setBusy(false);
    }
  };

  const cancelWithdrawal = async () => {
    const target = cancelTarget;
    if (!target || busy) return;
    setBusy(true);
    setError("");
    setNotice("");
    try {
      await cancelMarketProviderWithdrawal(target.id, target.version);
      setCancelTarget(null);
      await reload();
      setNotice(t("marketProviderFinance.withdrawalCancelled"));
    } catch (cause) {
      setCancelTarget(null);
      setError(cause instanceof ApiError && cause.status === 409 ? t("marketProviderFinance.withdrawalCancelChanged") : errorText(cause));
      await reload().catch(() => {});
    } finally {
      setBusy(false);
    }
  };

  const loadMore = async () => {
    if (busy || !nextCursor) return;
    setBusy(true);
    setError("");
    try {
      const page = await getMarketProviderWithdrawals(nextCursor);
      setWithdrawals((current) => [...current, ...page.items.filter((item) => !current.some((prior) => prior.id === item.id))]);
      setNextCursor(page.nextCursor);
    } catch (cause) {
      setError(errorText(cause));
    } finally {
      setBusy(false);
    }
  };

  const loadMoreLots = async () => {
    if (busy || !lotsCursor) return;
    setBusy(true);
    setError("");
    try {
      const page = await getMarketProviderEarnings(lotsCursor);
      setLots((current) => [...current, ...page.items.filter((item) => !current.some((prior) => prior.id === item.id))]);
      setLotsCursor(page.nextCursor);
    } catch (cause) {
      setError(errorText(cause));
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="mx-auto grid w-full max-w-5xl min-w-0 gap-4 px-4 pb-8 sm:px-6" aria-label={t("marketProviderFinance.title")}>
      <div className="grid gap-1">
        <h2 className="text-lg font-semibold">{t("marketProviderFinance.title")}</h2>
        <p className="text-sm text-muted-foreground">{t("marketProviderFinance.sevenDays")}</p>
        {walletMode !== "enabled" ? <p className="text-sm text-muted-foreground">{t("marketProviderFinance.notOpen")}</p> : null}
        {!finance.canWithdraw ? <p className="text-sm text-muted-foreground">{t("marketProviderFinance.readOnly")}</p> : null}
      </div>
      {error ? <p role="alert" className="rounded-md bg-destructive/5 px-3 py-2 text-sm text-destructive">{error}</p> : null}
      {notice ? <p role="status" className="rounded-md bg-emerald-50 px-3 py-2 text-sm text-emerald-800">{notice}</p> : null}
      <div className="grid gap-3 sm:grid-cols-3">
        {([
          ["pending", finance.summary.pendingUnits],
          ["withdrawable", finance.summary.withdrawableUnits],
          ["held", finance.summary.withdrawalHeldUnits],
        ] as const).map(([key, value]) => (
          <div key={key} className="rounded-lg border border-border bg-card p-4">
            <p className="text-xs text-muted-foreground">{t(`marketProviderFinance.${key}`)}</p>
            <p className="mt-1 break-all text-xl font-semibold tabular-nums">{formatWalletUnits(value, "$", locale)}</p>
          </div>
        ))}
      </div>

      {holds.length ? (
        <div className="grid gap-3 rounded-xl border border-amber-300 bg-amber-50/60 p-4" role="region" aria-labelledby="provider-holds-title">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <h3 id="provider-holds-title" className="flex items-center gap-2 font-semibold text-amber-950">
              <ShieldAlert className="size-4" aria-hidden="true" />{t("marketProviderFinance.holdsTitle")}
            </h3>
            <span className="text-sm font-semibold tabular-nums text-amber-950">{formatWalletUnits(totalHeld.toString(), "$", locale)}</span>
          </div>
          <p className="text-xs text-amber-900">{t("marketProviderFinance.holdsHelp")}</p>
          <ul className="grid gap-2">
            {holds.map((hold) => (
              <li key={`${hold.reasonKind}:${hold.reasonRef}`} className="flex min-w-0 flex-wrap items-center justify-between gap-2 rounded-lg bg-card px-3 py-2 text-sm">
                <div className="min-w-0">
                  <p className="font-medium">{t(holdReasonKey(hold.reasonKind))}</p>
                  <p className="text-xs text-muted-foreground">
                    {t("marketProviderFinance.holdLots", { count: hold.lots })} · {t("marketProviderFinance.holdSince", { time: dateText(hold.createdAt) })}
                  </p>
                </div>
                <span className="font-semibold tabular-nums">{formatWalletUnits(hold.heldUnits, "$", locale)}</span>
              </li>
            ))}
          </ul>
        </div>
      ) : null}

      <div className="grid min-w-0 gap-4 lg:grid-cols-2">
        <div className="grid content-start gap-4 rounded-lg border border-border bg-card p-4">
          <h3 className="font-semibold">{t("marketProviderFinance.destination")}</h3>
          {destination ? (
            <div className="grid gap-1 text-sm">
              <p className="flex min-w-0 items-center gap-2">
                <span className="min-w-0 break-all font-mono">{destination.address}</span>
                <CopyTextButton value={destination.address} size="sm" label={t("marketWallet.copyAddress")} />
              </p>
              <p className="text-muted-foreground">{addressReady
                ? t("marketProviderFinance.addressReady")
                : Number.isFinite(destinationReadyAt)
                  ? t("marketProviderFinance.addressCooling", { time: dateText(destination.usableAt) })
                  : t("marketProviderFinance.addressUnavailable")}</p>
            </div>
          ) : <p className="text-sm text-muted-foreground">{t("marketProviderFinance.noAddress")}</p>}
          {!challenge ? (
            <form className="grid gap-3" onSubmit={(event) => void sendCode(event)}>
              <WalletSectionLabel>{t("marketProviderFinance.stepOne")}</WalletSectionLabel>
              <label className="grid gap-1 text-sm">
                <span>{t("marketProviderFinance.tronAddress")}</span>
                <input value={address} onChange={(event) => { setAddress(event.target.value); setConfirmAddress(false); }}
                  autoComplete="off" spellCheck={false} aria-invalid={!!address && !addressValid}
                  aria-describedby="provider-address-help"
                  className={`${WALLET_INPUT} font-mono text-xs`} />
                <span id="provider-address-help" className="text-xs text-muted-foreground">{t("marketProviderFinance.addressTwoStepHelp")}</span>
              </label>
              <label className="flex items-start gap-2 text-xs text-muted-foreground">
                <input type="checkbox" className="mt-0.5" checked={confirmAddress} onChange={(event) => setConfirmAddress(event.target.checked)} />
                {t("marketProviderFinance.confirmAddress")}
              </label>
              <Button type="submit" size="sm" className="min-h-11" isDisabled={busy || !finance.canWithdraw || !addressValid || !confirmAddress}>
                {busy ? <Loader2 className="size-4 animate-spin motion-reduce:animate-none" aria-hidden="true" /> : <MailCheck className="size-4" aria-hidden="true" />}
                {t("marketProviderFinance.sendCode")}
              </Button>
            </form>
          ) : (
            <form className="grid gap-3" onSubmit={(event) => void confirmCode(event)}>
              <WalletSectionLabel pulse>{t("marketProviderFinance.stepTwo")}</WalletSectionLabel>
              <p className="text-sm">{t("marketProviderFinance.codeSentTo")}</p>
              <p className="break-all rounded-lg bg-muted px-3 py-2 font-mono text-xs">{challenge.address}</p>
              <label className="grid gap-1 text-sm">
                <span>{t("marketProviderFinance.code")}</span>
                <input ref={codeInputRef} value={code} onChange={(event) => setCode(event.target.value.replace(/\D/g, "").slice(0, 6))}
                  inputMode="numeric" autoComplete="one-time-code" maxLength={6} pattern="\d{6}"
                  aria-invalid={!!code && !codeValid} aria-describedby="provider-code-help"
                  className={`${WALLET_INPUT} text-center font-mono text-lg tracking-[0.5em]`} />
                <span id="provider-code-help" className={`text-xs tabular-nums ${challengeRemaining > 0 ? "text-muted-foreground" : "text-destructive"}`} aria-live="polite">
                  {challengeRemaining > 0
                    ? t("marketProviderFinance.codeExpiresIn", { time: `${Math.floor(challengeRemaining / 60)}:${String(challengeRemaining % 60).padStart(2, "0")}` })
                    : t("marketProviderFinance.codeExpired")}
                </span>
              </label>
              <div className="flex flex-wrap gap-2">
                <Button type="submit" size="sm" className="min-h-11" isDisabled={busy || !finance.canWithdraw || !codeValid || challengeRemaining <= 0}>
                  {t("marketProviderFinance.confirmChange")}
                </Button>
                <Button type="button" size="sm" variant="outline" className="min-h-11" isDisabled={busy || !finance.canWithdraw} onPress={() => void sendCode()}>
                  {t("marketProviderFinance.resendCode")}
                </Button>
                <Button type="button" size="sm" variant="ghost" className="min-h-11" isDisabled={busy} onPress={resetChallenge}>
                  {t("common.cancel")}
                </Button>
              </div>
            </form>
          )}
        </div>
        <div className="grid content-start gap-4 rounded-lg border border-border bg-card p-4">
          <h3 className="font-semibold">{t("marketProviderFinance.withdraw")}</h3>
          <p className="text-sm text-muted-foreground">{t("marketProviderFinance.manualPayment")}</p>
          <form className="grid gap-3" onSubmit={(event) => void requestWithdrawal(event)}>
            <label className="grid gap-1 text-sm">
              <span>{t("marketProviderFinance.amount")}</span>
              <input inputMode="decimal" value={amount} onChange={(event) => { setAmount(event.target.value); idempotencyRef.current = null; }}
                placeholder="10.00" aria-invalid={!!amount && !amountReady} aria-describedby="provider-withdraw-help"
                className={WALLET_INPUT} />
              <span id="provider-withdraw-help" className={`text-xs ${amountOverMax ? "text-destructive" : "text-muted-foreground"}`}>
                {amountOverMax ? t("marketProviderFinance.amountOverMax") : t("marketProviderFinance.amountLimits")}
              </span>
            </label>
            {tooManyOpen ? <p className="text-xs text-amber-800">{t("marketProviderFinance.tooManyOpen", { count: MAX_OPEN_WITHDRAWALS })}</p> : null}
            <Button type="submit" size="sm" className="min-h-11" isDisabled={busy || !finance.canWithdraw || walletMode !== "enabled" || !addressReady || !amountReady || tooManyOpen}>{t("marketProviderFinance.request")}</Button>
          </form>
          <div className="grid gap-2">
            <h4 className="text-sm font-semibold">{t("marketProviderFinance.history")}</h4>
            {withdrawals.length ? <ul className="grid gap-2">{withdrawals.map((item) => (
              <li key={item.id} className="grid min-w-0 gap-1 rounded-md bg-muted/50 px-3 py-2 text-xs">
                <div className="flex flex-wrap items-center justify-between gap-2">
                  <span className="font-medium tabular-nums">{formatWalletUnits(item.amountUnits, "$", locale)}</span>
                  <span className="flex items-center gap-2">
                    <span>{t(`marketProviderFinance.status.${item.status}` as Parameters<typeof t>[0])}</span>
                    {item.status === "requested" ? <Button size="sm" variant="outline" className="min-h-11 whitespace-nowrap" isDisabled={busy} onPress={() => setCancelTarget(item)}>
                      {t("marketProviderFinance.cancelWithdrawal")}
                    </Button> : null}
                  </span>
                </div>
                <span className="text-muted-foreground">{dateText(item.createdAt)}</span>
                {item.externalReference ? <TronTransactionLink hash={item.externalReference} /> : null}
              </li>
            ))}</ul> : <p className="text-sm text-muted-foreground">{t("marketProviderFinance.noHistory")}</p>}
            {nextCursor ? <Button size="sm" variant="ghost" isDisabled={busy} onPress={() => void loadMore()}>{t("marketWallet.more")}</Button> : null}
          </div>
        </div>
      </div>

      <div className="grid min-w-0 gap-3 rounded-lg border border-border bg-card p-4" role="region" aria-labelledby="provider-earnings-title">
        <div className="grid gap-1">
          <h3 id="provider-earnings-title" className="font-semibold">{t("marketProviderFinance.earningsTitle")}</h3>
          <p className="text-xs text-muted-foreground">{t("marketProviderFinance.earningsHelp")}</p>
        </div>
        {lots.length ? (
          <div className="overflow-x-auto">
            <table className="w-full min-w-[36rem] text-left text-xs">
              <thead className="text-muted-foreground">
                <tr className="border-b border-border">
                  <th scope="col" className="py-2 pr-3 font-medium">{t("marketProviderFinance.lot.fulfilled")}</th>
                  <th scope="col" className="py-2 pr-3 font-medium">{t("marketProviderFinance.lot.unlocks")}</th>
                  <th scope="col" className="py-2 pr-3 text-right font-medium">{t("marketProviderFinance.lot.amount")}</th>
                  <th scope="col" className="py-2 pr-3 text-right font-medium">{t("marketProviderFinance.lot.paidOut")}</th>
                  <th scope="col" className="py-2 font-medium">{t("marketProviderFinance.lot.state")}</th>
                </tr>
              </thead>
              <tbody>
                {lots.map((lot) => {
                  const state = lotState(lot, Date.now());
                  const reversed = units(lot.reversedUnits) > 0n;
                  return (
                    <tr key={lot.id} className="border-b border-border/60 last:border-0">
                      <td className="py-2 pr-3">{dateText(lot.fulfilledAt)}</td>
                      <td className="py-2 pr-3">{dateText(lot.unlockAt)}</td>
                      <td className="py-2 pr-3 text-right tabular-nums">
                        {formatWalletUnits(lot.amountUnits, "$", locale)}
                        {reversed ? <span className="block text-[0.7rem] text-muted-foreground">{t("marketProviderFinance.lot.reversedAmount", { amount: formatWalletUnits(lot.reversedUnits, "$", locale) })}</span> : null}
                      </td>
                      <td className="py-2 pr-3 text-right tabular-nums">{formatWalletUnits(lot.paidUnits, "$", locale)}</td>
                      <td className="py-2"><WalletBadge tone={state.tone}>{t(state.key)}</WalletBadge></td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        ) : <p className="text-sm text-muted-foreground">{t("marketProviderFinance.noEarnings")}</p>}
        {lotsCursor ? <Button size="sm" variant="ghost" className="justify-self-start" isDisabled={busy} onPress={() => void loadMoreLots()}>{t("marketWallet.more")}</Button> : null}
      </div>

      <ConfirmAlertDialog
        open={!!cancelTarget}
        title={t("marketProviderFinance.cancelWithdrawalTitle")}
        description={t("marketProviderFinance.cancelWithdrawalNote", { amount: cancelTarget ? formatWalletUnits(cancelTarget.amountUnits, "$", locale) : "" })}
        confirmLabel={t("marketProviderFinance.cancelWithdrawal")}
        cancelLabel={t("common.close")}
        tone="warning"
        busy={busy}
        onConfirm={() => void cancelWithdrawal()}
        onOpenChange={(open) => { if (!open && !busy) setCancelTarget(null); }}
      />
    </section>
  );
}
