"use client";

/* Hallmark · component: wallet transfer · genre: modern-minimal · theme: existing Router system
 * states: default · hover · focus · active · disabled · loading · error · success
 * pre-emit critique: P4 H5 E4 S5 R5 V4
 */
import * as React from "react";
import { Button } from "@heroui/react";
import { ArrowRightLeft, Loader2 } from "lucide-react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { ApiError } from "@/lib/api";
import {
  formatWalletUnits,
  parseNonNegativeUsdMinor,
} from "@/lib/market-wallet-money";
import { WALLET_INPUT } from "@/components/market/wallet-primitives";
import {
  confirmWalletTransfer,
  getTransferCapacity,
  quoteWalletTransfer,
  sendWalletTransferCode,
  type TransferCapacity,
  type TransferQuote,
  type TransferChallenge,
} from "@/lib/wallet-policy-api";

export function WalletTransferPanel({
  onTransferred,
  walletVersion,
}: {
  onTransferred: () => Promise<void>;
  walletVersion: number;
}) {
  const { t } = useLocaleText();
  const [capacity, setCapacity] = React.useState<TransferCapacity | null>(null);
  const [recipient, setRecipient] = React.useState("");
  const [amount, setAmount] = React.useState("");
  const [all, setAll] = React.useState(false);
  const [quote, setQuote] = React.useState<TransferQuote | null>(null);
  const [challenge, setChallenge] = React.useState<TransferChallenge | null>(
    null,
  );
  const [code, setCode] = React.useState("");
  const [busy, setBusy] = React.useState("");
  const [error, setError] = React.useState("");
  const [success, setSuccess] = React.useState("");
  const [now, setNow] = React.useState(Date.now());
  const [resendAt, setResendAt] = React.useState(0);
  const [reload, setReload] = React.useState(0);
  const codeRef = React.useRef<HTMLInputElement>(null);
  const dialog = React.useRef<HTMLDialogElement>(null);
  const busyRef = React.useRef(false);
  const keyRef = React.useRef("");
  const amountMinor = parseNonNegativeUsdMinor(amount);
  const available = capacity?.transferableUnits ?? "0";

  React.useEffect(() => {
    let active = true;
    getTransferCapacity()
      .then((value) => {
        if (active) setCapacity(value);
      })
      .catch((cause) => {
        if (active)
          setError(cause instanceof Error ? cause.message : String(cause));
      });
    return () => {
      active = false;
    };
  }, [walletVersion, reload]);
  React.useEffect(() => {
    if (challenge) codeRef.current?.focus();
  }, [challenge]);
  React.useEffect(() => {
    if (quote && dialog.current && !dialog.current.open)
      dialog.current.showModal();
    if (!quote && dialog.current?.open) dialog.current.close();
  }, [quote]);
  React.useEffect(() => {
    if (!quote) return;
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [quote]);
  const fail = (cause: unknown) => {
    const key = cause instanceof ApiError ? cause.code : null;
    setError(
      key === "WALLET_TRANSFER_CODE_INVALID"
        ? t("walletTransfer.badCode")
        : key === "WALLET_TRANSFER_INSUFFICIENT"
          ? t("walletTransfer.insufficient")
          : key === "WALLET_TRANSFER_USER_UNVERIFIED"
            ? t("walletTransfer.verify")
            : key === "WALLET_TRANSFER_RECIPIENT_UNAVAILABLE"
              ? t("walletTransfer.recipientUnavailable")
              : key === "WALLET_TRANSFER_PAUSED"
                ? t("walletTransfer.paused")
                : key === "WALLET_TRANSFER_QUOTE_EXPIRED" ||
                    key === "WALLET_TRANSFER_CODE_EXPIRED"
                  ? t("walletTransfer.expired")
                  : cause instanceof Error
                    ? cause.message
                    : String(cause),
    );
  };
  const run = async (name: string, action: () => Promise<void>) => {
    if (busyRef.current) return;
    busyRef.current = true;
    setBusy(name);
    setError("");
    try {
      await action();
    } catch (cause) {
      fail(cause);
    } finally {
      busyRef.current = false;
      setBusy("");
    }
  };
  const close = () => {
    if (!busyRef.current) {
      setQuote(null);
      setChallenge(null);
      setCode("");
      setError("");
    }
  };
  const review = (event: React.FormEvent) => {
    event.preventDefault();
    setSuccess("");
    void run("quote", async () => {
      const value = await quoteWalletTransfer(
        recipient.trim(),
        all ? null : amountMinor,
        all,
      );
      keyRef.current = crypto.randomUUID();
      setChallenge(null);
      setCode("");
      setNow(Date.now());
      setResendAt(0);
      setQuote(value);
    });
  };
  const send = () =>
    void run("code", async () => {
      if (!quote) return;
      const value = await sendWalletTransferCode(quote.id);
      setChallenge(value);
      setCode("");
      setNow(Date.now());
      setResendAt(Date.now() + value.resendAfterSeconds * 1000);
    });
  const confirm = (event: React.FormEvent) => {
    event.preventDefault();
    void run("confirm", async () => {
      if (!quote || !challenge) return;
      const value = await confirmWalletTransfer(
        quote.id,
        challenge.id,
        code.trim(),
        keyRef.current,
      );
      setSuccess(
        t("walletTransfer.success", {
          amount: formatWalletUnits(value.amountUnits),
          email: value.recipientEmail,
        }),
      );
      setQuote(null);
      setChallenge(null);
      setCode("");
      setAmount("");
      setAll(false);
      // Receipts and view refreshes are not allowed to repeat the money action.
      await onTransferred().catch(() => {});
      try {
        setCapacity(await getTransferCapacity());
      } catch {
        setCapacity(null);
        setError(t("walletTransfer.refreshFailed"));
      }
    });
  };
  const expired = !!quote && Date.parse(quote.expiresAt) <= now;
  const codeExpired = !!challenge && Date.parse(challenge.expiresAt) <= now;
  const wait = Math.max(0, Math.ceil((resendAt - now) / 1000));
  const enabled = capacity?.enabled && capacity.verified;
  return (
    <section id="wallet-transfer" className="grid min-w-0 gap-4 rounded-xl border border-border bg-card p-5">
      <div>
        <h2 className="flex items-center gap-2 text-base font-semibold">
          <ArrowRightLeft className="size-4" aria-hidden />
          {t("walletTransfer.title")}
        </h2>
        <p className="mt-1 text-sm leading-6 text-muted-foreground">
          {t("walletTransfer.help")}
        </p>
      </div>
      {!quote && error ? (
        <p role="alert" className="break-words text-sm text-destructive">
          {error}
        </p>
      ) : null}
      {success ? (
        <p role="status" className="break-words text-sm">
          {success}
        </p>
      ) : null}
      {!capacity ? (
        error ? (
          <Button
            variant="outline"
            className="min-h-11 justify-self-start"
            onPress={() => {
              setError("");
              setReload((value) => value + 1);
            }}
          >
            {t("common.reload")}
          </Button>
        ) : (
          <p className="text-sm text-muted-foreground">{t("common.loading")}</p>
        )
      ) : !enabled ? (
        <p className="text-sm text-muted-foreground">
          {t(
            capacity.blockers?.[0] === "email_unavailable" ? "walletTransfer.emailUnavailable"
              : capacity.blockers?.[0] === "finance_hold" ? "walletTransfer.financeHold"
              : !capacity.verified
              ? "walletTransfer.verify"
              : "walletTransfer.paused",
          )}
        </p>
      ) : (
        <form onSubmit={review} className="grid min-w-0 gap-4 sm:grid-cols-2">
          <label className="grid min-w-0 gap-1 text-sm">
            <span>{t("walletTransfer.email")}</span>
            <input
              type="email"
              autoComplete="off"
              required
              maxLength={254}
              className={WALLET_INPUT}
              value={recipient}
              onChange={(event) => setRecipient(event.target.value)}
              disabled={!!busy}
            />
          </label>
          <label className="grid min-w-0 gap-1 text-sm">
            <span>{t("walletTransfer.amount")}</span>
            <input
              inputMode="decimal"
              autoComplete="off"
              className={WALLET_INPUT}
              value={
                all ? formatWalletUnits(available).replace(/^\$/, "") : amount
              }
              onChange={(event) => setAmount(event.target.value)}
              disabled={!!busy || all}
              required={!all}
            />
          </label>
          <div className="grid gap-1 sm:col-span-2">
            <label className="flex min-h-11 items-center gap-2 text-sm">
              <input
                type="checkbox"
                checked={all}
                onChange={(event) => setAll(event.target.checked)}
                disabled={!!busy}
                className="size-4 accent-primary"
              />
              {t("walletTransfer.all")}
            </label>
            <p className="text-sm tabular-nums">
              {t("walletTransfer.available", {
                amount: formatWalletUnits(available),
              })}
            </p>
            <p className="text-xs leading-5 text-muted-foreground">
              {t("walletTransfer.commitment")}
            </p>
          </div>
          <Button
            type="submit"
            variant="primary"
            className="min-h-11 justify-self-start whitespace-nowrap"
            isDisabled={
              !!busy ||
              !recipient.trim() ||
              BigInt(available) <= 0n ||
              (!all && (amountMinor == null || amountMinor <= 0))
            }
          >
            {busy === "quote" ? (
              <Loader2 className="size-4 animate-spin motion-reduce:animate-none" />
            ) : null}
            {t("walletTransfer.preview")}
          </Button>
        </form>
      )}
      <dialog
        ref={dialog}
        onCancel={(event) => {
          event.preventDefault();
          close();
        }}
        onClick={(event) => {
          if (event.target === event.currentTarget) close();
        }}
        className="fixed inset-0 m-auto h-fit max-h-[85dvh] w-[calc(100%-2rem)] max-w-md overflow-y-auto rounded-xl bg-card p-6 text-card-foreground shadow-lg backdrop:bg-foreground/30"
        aria-labelledby="wallet-transfer-title"
      >
        {quote ? (
          <div className="grid min-w-0 gap-5">
            <h3 id="wallet-transfer-title" className="text-lg font-semibold">
              {t("walletTransfer.review")}
            </h3>
            <div className="grid gap-2">
              <p className="break-all text-base font-medium">
                {quote.recipientEmail}
              </p>
              <p className="break-all text-3xl font-semibold tabular-nums">
                {formatWalletUnits(quote.amountUnits)}
              </p>
              {BigInt(quote.amountUnits) % 86_400n !== 0n ? (
                <p className="break-all text-xs text-muted-foreground">
                  {t("walletTransfer.exact", { units: quote.amountUnits })}
                </p>
              ) : null}
              <p className="text-xs leading-5 text-muted-foreground">
                {t("walletTransfer.final")}
              </p>
            </div>
            {error ? (
              <p role="alert" className="break-words text-sm text-destructive">
                {error}
              </p>
            ) : null}
            {expired || codeExpired ? (
              <p role="status" className="text-sm text-destructive">
                {t("walletTransfer.expired")}
              </p>
            ) : null}
            {challenge ? (
              <form onSubmit={confirm} className="grid min-w-0 gap-3">
                <label className="grid min-w-0 gap-1 text-sm">
                  <span>{t("walletTransfer.code")}</span>
                  <input
                    ref={codeRef}
                    autoComplete="one-time-code"
                    inputMode="numeric"
                    maxLength={6}
                    pattern="[0-9]{6}"
                    required
                    value={code}
                    onChange={(event) =>
                      setCode(event.target.value.replace(/\D/g, ""))
                    }
                    className={WALLET_INPUT}
                    disabled={!!busy}
                  />
                  <span className="break-all text-xs leading-5 text-muted-foreground">
                    {t("walletTransfer.codeHelp", { email: quote.senderEmail })}
                  </span>
                </label>
                <Button
                  type="submit"
                  variant="primary"
                  className="min-h-11 whitespace-nowrap"
                  isDisabled={
                    !!busy || expired || codeExpired || !/^\d{6}$/.test(code)
                  }
                >
                  {busy === "confirm" ? (
                    <Loader2 className="size-4 animate-spin motion-reduce:animate-none" />
                  ) : null}
                  {t("walletTransfer.confirm", {
                    amount: formatWalletUnits(quote.amountUnits),
                  })}
                </Button>
                <Button
                  variant="ghost"
                  className="min-h-11 whitespace-nowrap"
                  onPress={send}
                  isDisabled={!!busy || expired || wait > 0}
                >
                  {t(
                    wait
                      ? "walletTransfer.resendWait"
                      : "walletTransfer.resend",
                    { seconds: wait },
                  )}
                </Button>
              </form>
            ) : (
              <Button
                variant="primary"
                className="min-h-11 whitespace-nowrap"
                onPress={send}
                isDisabled={!!busy || expired}
              >
                {busy === "code" ? (
                  <Loader2 className="size-4 animate-spin motion-reduce:animate-none" />
                ) : null}
                {t("walletTransfer.sendCode")}
              </Button>
            )}
            <Button
              variant="ghost"
              className="min-h-11 whitespace-nowrap"
              isDisabled={!!busy}
              onPress={close}
            >
              {t("common.close")}
            </Button>
          </div>
        ) : null}
      </dialog>
    </section>
  );
}
