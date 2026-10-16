"use client";

import * as React from "react";
import { Button } from "@heroui/react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import {
  getMarketProviderFinance,
  getMarketProviderWithdrawals,
  requestMarketProviderWithdrawal,
  saveMarketProviderDestination,
  type MarketProviderFinance,
  type MarketProviderWithdrawal,
} from "@/lib/market-wallet-api";
import { formatWalletUnits, parseWithdrawalMinor } from "@/lib/market-wallet-money";

const UNITS_PER_MINOR = 86_400n;

function errorText(cause: unknown) {
  return cause instanceof Error ? cause.message : String(cause);
}

export function ProviderFinancePanel({ walletMode }: { walletMode: "disabled" | "shadow" | "enabled" }) {
  const { locale, t } = useLocaleText();
  const [finance, setFinance] = React.useState<MarketProviderFinance | null>(null);
  const [withdrawals, setWithdrawals] = React.useState<MarketProviderWithdrawal[]>([]);
  const [nextCursor, setNextCursor] = React.useState<string | null>(null);
  const [address, setAddress] = React.useState("");
  const [confirmAddress, setConfirmAddress] = React.useState(false);
  const [amount, setAmount] = React.useState("");
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState("");
  const [notice, setNotice] = React.useState("");
  const idempotencyRef = React.useRef<{ amountMinor: number; key: string } | null>(null);

  const reload = React.useCallback(async () => {
    const [nextFinance, page] = await Promise.all([
      getMarketProviderFinance(), getMarketProviderWithdrawals(),
    ]);
    setFinance(nextFinance);
    setWithdrawals(page.items);
    setNextCursor(page.nextCursor);
  }, []);

  React.useEffect(() => {
    let active = true;
    Promise.all([getMarketProviderFinance(), getMarketProviderWithdrawals()])
      .then(([nextFinance, page]) => {
        if (!active) return;
        setFinance(nextFinance);
        setWithdrawals(page.items);
        setNextCursor(page.nextCursor);
      })
      .catch((cause) => { if (active) setError(errorText(cause)); });
    return () => { active = false; };
  }, []);

  if (!finance) return error ? <p role="alert" className="mx-auto max-w-5xl px-4 text-sm text-destructive">{error}</p> : null;
  if (!finance.isProvider) return null;

  const destination = finance.destination;
  const destinationReadyAt = destination ? new Date(destination.usableAt).getTime() : Number.NaN;
  const addressReady = destination && Number.isFinite(destinationReadyAt) && destinationReadyAt <= Date.now();
  const amountMinor = parseWithdrawalMinor(amount);
  const amountReady = amountMinor != null
    && BigInt(amountMinor) * UNITS_PER_MINOR <= BigInt(finance.summary.withdrawableUnits);

  const saveAddress = async (event: React.FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (busy || !confirmAddress || !address.trim()) return;
    setBusy(true);
    setError("");
    setNotice("");
    try {
      await saveMarketProviderDestination(address.trim());
      await reload();
      setAddress("");
      setConfirmAddress(false);
      setNotice(t("marketProviderFinance.addressSaved"));
    } catch (cause) {
      setError(errorText(cause));
    } finally {
      setBusy(false);
    }
  };

  const requestWithdrawal = async (event: React.FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (busy || walletMode !== "enabled" || !addressReady || !amountReady || amountMinor == null) return;
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

  return (
    <section className="mx-auto grid w-full max-w-5xl min-w-0 gap-4 px-4 pb-8 sm:px-6" aria-label={t("marketProviderFinance.title")}>
      <div className="grid gap-1">
        <h2 className="text-lg font-semibold">{t("marketProviderFinance.title")}</h2>
        <p className="text-sm text-muted-foreground">{t("marketProviderFinance.sevenDays")}</p>
        {walletMode !== "enabled" ? <p className="text-sm text-muted-foreground">{t("marketProviderFinance.notOpen")}</p> : null}
      </div>
      {error ? <p role="alert" className="rounded-md bg-destructive/5 px-3 py-2 text-sm text-destructive">{error}</p> : null}
      {notice ? <p role="status" className="rounded-md bg-emerald-50 px-3 py-2 text-sm text-emerald-800">{notice}</p> : null}
      <div className="grid gap-3 sm:grid-cols-3">
        {([
          ["pending", finance.summary.pendingUnits],
          ["withdrawable", finance.summary.withdrawableUnits],
          ["held", finance.summary.withdrawalHeldUnits],
        ] as const).map(([key, units]) => (
          <div key={key} className="rounded-lg border border-border bg-card p-4">
            <p className="text-xs text-muted-foreground">{t(`marketProviderFinance.${key}`)}</p>
            <p className="mt-1 break-all text-xl font-semibold tabular-nums">{formatWalletUnits(units, "$", locale)}</p>
          </div>
        ))}
      </div>
      <div className="grid min-w-0 gap-4 lg:grid-cols-2">
        <div className="grid content-start gap-4 rounded-lg border border-border bg-card p-4">
          <h3 className="font-semibold">{t("marketProviderFinance.destination")}</h3>
          {destination ? (
            <div className="grid gap-1 text-sm">
              <p className="break-all font-mono">{destination.address}</p>
              <p className="text-muted-foreground">{addressReady
                ? t("marketProviderFinance.addressReady")
                : Number.isFinite(destinationReadyAt)
                  ? t("marketProviderFinance.addressCooling", {
                    time: new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short" }).format(new Date(destinationReadyAt)),
                  })
                  : t("marketProviderFinance.addressUnavailable")}</p>
            </div>
          ) : <p className="text-sm text-muted-foreground">{t("marketProviderFinance.noAddress")}</p>}
          <form className="grid gap-3" onSubmit={(event) => void saveAddress(event)}>
            <label className="grid gap-1 text-sm">
              <span>{t("marketProviderFinance.tronAddress")}</span>
              <input value={address} onChange={(event) => { setAddress(event.target.value); setConfirmAddress(false); }}
                autoComplete="off" spellCheck={false} className="h-11 min-w-0 rounded-md border border-input bg-background px-3 font-mono text-xs focus-visible:outline-2 focus-visible:outline-ring" />
            </label>
            <label className="flex items-start gap-2 text-xs text-muted-foreground">
              <input type="checkbox" className="mt-0.5" checked={confirmAddress} onChange={(event) => setConfirmAddress(event.target.checked)} />
              {t("marketProviderFinance.confirmAddress")}
            </label>
            <Button type="submit" size="sm" isDisabled={busy || !address.trim() || !confirmAddress}>{t("marketProviderFinance.saveAddress")}</Button>
          </form>
        </div>
        <div className="grid content-start gap-4 rounded-lg border border-border bg-card p-4">
          <h3 className="font-semibold">{t("marketProviderFinance.withdraw")}</h3>
          <p className="text-sm text-muted-foreground">{t("marketProviderFinance.manualPayment")}</p>
          <form className="grid gap-3" onSubmit={(event) => void requestWithdrawal(event)}>
            <label className="grid gap-1 text-sm">
              <span>{t("marketProviderFinance.amount")}</span>
              <input inputMode="decimal" value={amount} onChange={(event) => { setAmount(event.target.value); idempotencyRef.current = null; }}
                placeholder="10.00" aria-invalid={!!amount && !amountReady}
                className="h-11 min-w-0 rounded-md border border-input bg-background px-3 focus-visible:outline-2 focus-visible:outline-ring aria-invalid:border-destructive" />
              <span className="text-xs text-muted-foreground">{t("marketProviderFinance.amountHelp")}</span>
            </label>
            <Button type="submit" size="sm" isDisabled={busy || walletMode !== "enabled" || !addressReady || !amountReady}>{t("marketProviderFinance.request")}</Button>
          </form>
          <div className="grid gap-2">
            <h4 className="text-sm font-semibold">{t("marketProviderFinance.history")}</h4>
            {withdrawals.length ? <ul className="grid gap-2">{withdrawals.map((item) => (
              <li key={item.id} className="flex flex-wrap items-center justify-between gap-2 rounded-md bg-muted/50 px-3 py-2 text-xs">
                <span className="tabular-nums">{formatWalletUnits(item.amountUnits, "$", locale)}</span>
                <span>{t(`marketProviderFinance.status.${item.status}` as Parameters<typeof t>[0])}</span>
              </li>
            ))}</ul> : <p className="text-sm text-muted-foreground">{t("marketProviderFinance.noHistory")}</p>}
            {nextCursor ? <Button size="sm" variant="ghost" isDisabled={busy} onPress={() => void loadMore()}>{t("marketWallet.more")}</Button> : null}
          </div>
        </div>
      </div>
    </section>
  );
}
