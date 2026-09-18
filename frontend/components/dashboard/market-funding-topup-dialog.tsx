"use client";

import * as React from "react";
import { Button, Modal, toast } from "@heroui/react";
import { AlertTriangle, Ban, CircleCheckBig, Copy, Loader2, RefreshCw } from "lucide-react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import {
  cancelBinanceFundingIntent,
  createBinanceFundingIntent,
  getBinanceFundingIntent,
  refreshBinanceFundingIntent,
} from "@/lib/api";
import type { BinanceFundingIntent, MarketFundingSummary } from "@/lib/types";
import { formatUsdMoney } from "@/lib/market-money";

function statusLabel(status: string, t: ReturnType<typeof useLocaleText>["t"]) {
  switch (status) {
    case "pending": return t("marketBilling.binance.status.pending");
    case "credited": return t("marketBilling.prepaid.topup.status.credited");
    case "expired": return t("marketBilling.binance.status.expired");
    case "cancelled": return t("marketBilling.binance.status.cancelled");
    case "review_required": return t("marketBilling.binance.status.review");
    default: return status.replaceAll("_", " ");
  }
}

export function MarketFundingSummaryCard({
  funding,
  className = "",
}: {
  funding: MarketFundingSummary;
  className?: string;
}) {
  const { locale, t } = useLocaleText();
  const credit = funding.creditKind === "unlimited"
    ? t("marketFunding.credit.unlimited")
    : funding.creditKind === "none"
      ? t("marketFunding.credit.none")
      : formatUsdMoney(funding.creditAvailableMinor || 0, locale);

  return (
    <section className={`grid gap-3 rounded-md border border-slate-200 bg-slate-50 p-3 ${className}`.trim()}>
      <div className="flex flex-wrap items-start justify-between gap-2">
        <div className="min-w-0">
          <strong className="block text-sm">{t("marketFunding.title")}</strong>
          <span className="block break-all text-xs text-slate-500">{funding.supplierEmail}</span>
        </div>
        <span className="rounded-full bg-white px-2 py-1 text-[11px] text-slate-600 ring-1 ring-slate-200">
          {funding.fundingMode === "prepaid" ? t("marketFunding.mode.prepaid") : t("marketFunding.mode.hybrid")}
        </span>
      </div>
      <div className="grid grid-cols-2 gap-x-4 gap-y-2 text-sm sm:grid-cols-4">
        <div><span className="block text-xs text-slate-500">{t("marketFunding.prepaidAvailable")}</span><strong className="tabular-nums">{formatUsdMoney(funding.prepaidAvailableMinor, locale)}</strong></div>
        <div><span className="block text-xs text-slate-500">{t("marketFunding.creditAvailable")}</span><strong className="tabular-nums">{credit}</strong></div>
        <div><span className="block text-xs text-slate-500">{t("marketFunding.coverage")}</span><strong className="tabular-nums">{formatUsdMoney(funding.requiredCoverageMinor, locale)}</strong></div>
        <div><span className="block text-xs text-slate-500">{t("marketFunding.shortfall")}</span><strong className={`tabular-nums ${funding.requiredTopupMinor > 0 ? "text-rose-700" : "text-emerald-700"}`}>{formatUsdMoney(funding.requiredTopupMinor, locale)}</strong></div>
      </div>
      <p className="text-xs leading-5 text-slate-600">{t("marketFunding.policy")}</p>
    </section>
  );
}

export function MarketFundingTopupDialog({
  open,
  funding,
  onClose,
  onCredited,
}: {
  open: boolean;
  funding?: MarketFundingSummary;
  onClose: () => void;
  onCredited: (intent: BinanceFundingIntent) => void | Promise<void>;
}) {
  const { locale, t } = useLocaleText();
  const [amount, setAmount] = React.useState("");
  const [intent, setIntent] = React.useState<BinanceFundingIntent | null>(null);
  const [error, setError] = React.useState("");
  const [busy, setBusy] = React.useState("");
  const [clockMs, setClockMs] = React.useState(() => Date.now());
  const idempotencyRef = React.useRef<{ amountMinor: number; key: string } | null>(null);
  const creditedRef = React.useRef("");
  const intentEpochRef = React.useRef(0);
  const intentMutationRef = React.useRef(false);
  const intentPollInFlightRef = React.useRef(false);

  React.useEffect(() => {
    if (!open) return;
    intentEpochRef.current += 1;
    intentMutationRef.current = false;
    intentPollInFlightRef.current = false;
    setAmount(((funding?.requiredTopupMinor || 1) / 100).toFixed(2));
    setIntent(null);
    setError("");
    setBusy("");
    setClockMs(Date.now());
    idempotencyRef.current = null;
    creditedRef.current = "";
  }, [funding?.requiredTopupMinor, funding?.supplierUserId, open]);

  React.useEffect(() => {
    if (!open || !intent?.id || intent.status !== "pending") return;
    const controller = new AbortController();
    let active = true;
    const intentId = intent.id;
    const epoch = ++intentEpochRef.current;
    intentPollInFlightRef.current = false;
    const accept = (next: BinanceFundingIntent) => {
      if (!active || intentEpochRef.current !== epoch) return;
      setIntent(next);
      setError("");
    };
    const poll = window.setInterval(() => {
      if (intentMutationRef.current || intentPollInFlightRef.current) return;
      intentPollInFlightRef.current = true;
      void getBinanceFundingIntent(intentId, controller.signal)
        .then(accept)
        .catch((reason) => {
          if (!active || controller.signal.aborted || intentEpochRef.current !== epoch) return;
          setIntent((current) => current?.id === intentId
            ? { ...current, accountStatus: "unavailable" }
            : current);
          setError(reason instanceof Error ? reason.message : String(reason));
        })
        .finally(() => {
          if (active && intentEpochRef.current === epoch) {
            intentPollInFlightRef.current = false;
          }
        });
    }, 3_000);
    const clock = window.setInterval(() => setClockMs(Date.now()), 1_000);
    return () => {
      active = false;
      controller.abort();
      window.clearInterval(poll);
      window.clearInterval(clock);
      intentPollInFlightRef.current = false;
    };
  }, [intent?.id, intent?.status, open]);

  React.useEffect(() => {
    if (!open || intent?.status !== "credited" || creditedRef.current === intent.id) return;
    creditedRef.current = intent.id;
    toast.success(t("marketBilling.prepaid.topup.credited"));
    void onCredited(intent);
  }, [intent?.id, intent?.status, onCredited, open, t]);

  const amountMinor = React.useMemo(() => {
    const normalized = amount.trim();
    if (!/^\d+(?:\.\d{1,2})?$/.test(normalized)) return null;
    const value = Number(normalized);
    const minor = Math.round(value * 100);
    return Number.isSafeInteger(minor) && minor > 0 ? minor : null;
  }, [amount]);
  const remainingSeconds = intent
    ? Math.max(0, Math.floor((Date.parse(intent.expiresAt) - clockMs) / 1_000))
    : 0;
  const canTransfer = intent?.status === "pending"
    && intent.accountStatus === "verified"
    && remainingSeconds > 0;
  const countdown = `${String(Math.floor(remainingSeconds / 60)).padStart(2, "0")}:${String(remainingSeconds % 60).padStart(2, "0")}`;

  const createIntent = async () => {
    if (!funding || amountMinor == null || busy) return;
    if (amountMinor < funding.requiredTopupMinor) {
      setError(t("marketFunding.topup.minimum", { amount: formatUsdMoney(funding.requiredTopupMinor, locale) }));
      return;
    }
    if (!idempotencyRef.current || idempotencyRef.current.amountMinor !== amountMinor) {
      idempotencyRef.current = { amountMinor, key: `market-funding:${crypto.randomUUID()}` };
    }
    const mutationEpoch = ++intentEpochRef.current;
    intentMutationRef.current = true;
    intentPollInFlightRef.current = false;
    setBusy("create");
    setError("");
    try {
      const next = await createBinanceFundingIntent({
        supplierUserId: funding.supplierUserId,
        amountMinor,
        idempotencyKey: idempotencyRef.current.key,
      });
      if (intentEpochRef.current !== mutationEpoch) return;
      setIntent(next);
      setClockMs(Date.now());
    } catch (reason) {
      if (intentEpochRef.current !== mutationEpoch) return;
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      if (intentEpochRef.current === mutationEpoch) {
        intentMutationRef.current = false;
        setBusy("");
      }
    }
  };

  const refreshIntent = async () => {
    if (!intent || busy || !window.confirm(t("marketBilling.binance.refreshConfirm"))) return;
    const mutationEpoch = ++intentEpochRef.current;
    intentMutationRef.current = true;
    intentPollInFlightRef.current = false;
    setBusy("refresh");
    try {
      const next = await refreshBinanceFundingIntent(intent.id);
      if (intentEpochRef.current !== mutationEpoch) return;
      setIntent(next);
      idempotencyRef.current = null;
      setError("");
      setClockMs(Date.now());
    } catch (reason) {
      if (intentEpochRef.current !== mutationEpoch) return;
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      if (intentEpochRef.current === mutationEpoch) {
        intentMutationRef.current = false;
        setBusy("");
      }
    }
  };

  const cancelIntent = async () => {
    if (!intent || busy || !window.confirm(t("marketBilling.binance.cancelConfirm"))) return;
    const mutationEpoch = ++intentEpochRef.current;
    intentMutationRef.current = true;
    intentPollInFlightRef.current = false;
    setBusy("cancel");
    try {
      const next = await cancelBinanceFundingIntent(intent.id);
      if (intentEpochRef.current !== mutationEpoch) return;
      setIntent(next);
      setError("");
    } catch (reason) {
      if (intentEpochRef.current !== mutationEpoch) return;
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      if (intentEpochRef.current === mutationEpoch) {
        intentMutationRef.current = false;
        setBusy("");
      }
    }
  };

  const copy = async (value: string) => {
    try {
      await navigator.clipboard.writeText(value);
      toast.success(t("marketBilling.binance.copied"));
    } catch {
      toast.danger(t("marketBilling.binance.copyFailed"));
    }
  };

  return (
    <Modal.Backdrop isOpen={open} onOpenChange={(next) => !next && !busy && onClose()}>
      <Modal.Container placement="center">
        <Modal.Dialog className="light w-[min(560px,calc(100vw-2rem))] max-w-none !bg-white !text-slate-900">
          <Modal.Header><Modal.Heading>{t("marketBilling.dialog.topup")}</Modal.Heading></Modal.Header>
          <Modal.Body className="grid max-h-[70vh] gap-4 overflow-y-auto">
            {funding ? (
              <div className="grid grid-cols-2 gap-3 rounded-md border border-slate-200 bg-slate-50 p-3 text-sm">
                <div><span className="block text-xs text-slate-500">{t("marketBilling.supplier")}</span><strong className="break-all">{funding.supplierEmail}</strong></div>
                <div><span className="block text-xs text-slate-500">{t("marketFunding.shortfall")}</span><strong>{formatUsdMoney(funding.requiredTopupMinor, locale)}</strong></div>
              </div>
            ) : null}
            {!intent ? (
              <label className="grid gap-1 text-sm">
                <span className="text-slate-500">{t("marketBilling.prepaid.amountUsd")}</span>
                <input value={amount} onChange={(event) => { setAmount(event.target.value); idempotencyRef.current = null; }} inputMode="decimal" className="h-10 rounded-md border border-slate-200 px-3 tabular-nums" autoFocus />
              </label>
            ) : null}
            {error ? <div className="flex gap-2 rounded-md border border-rose-200 bg-rose-50 p-3 text-xs leading-5 text-rose-800"><AlertTriangle className="mt-0.5 h-4 w-4 shrink-0" />{error}</div> : null}
            {intent ? (
              <>
                <div className={`rounded-xl border p-4 text-center ${intent.status === "credited" ? "border-emerald-200 bg-emerald-50" : "border-amber-200 bg-amber-50/70"}`}>
                  {intent.status === "credited" ? <CircleCheckBig className="mx-auto h-8 w-8 text-emerald-600" /> : null}
                  <p className="text-xs font-medium text-slate-500">{canTransfer ? t("marketBilling.binance.exactAmount") : statusLabel(intent.status, t)}</p>
                  <strong className="mt-1 block text-3xl tabular-nums">{intent.payAmount}<span className="ml-2 text-base">{intent.asset}</span></strong>
                  {canTransfer ? <Button size="sm" variant="outline" className="mt-3" onClick={() => void copy(intent.payAmount)}><Copy className="h-4 w-4" />{t("marketBilling.binance.copyAmount")}</Button> : null}
                </div>
                <div className="divide-y divide-dashed divide-slate-200 rounded-lg border border-slate-200 px-3 text-sm">
                  {canTransfer ? <>
                    <div className="flex items-center justify-between gap-3 py-3"><span className="text-slate-500">{t("marketBilling.binance.receiverUid")}</span><span className="flex items-center gap-2"><strong className="break-all text-right">{intent.receiverUid}</strong><Button isIconOnly size="sm" variant="ghost" aria-label={t("marketBilling.binance.copyUid")} onClick={() => void copy(intent.receiverUid)}><Copy className="h-4 w-4" /></Button></span></div>
                    <div className="flex items-center justify-between gap-3 py-3"><span className="text-slate-500">{t("marketBilling.binance.noteCode")}</span><span className="flex items-center gap-2"><strong>{intent.noteCode}</strong><Button isIconOnly size="sm" variant="ghost" aria-label={t("marketBilling.binance.copyNote")} onClick={() => void copy(intent.noteCode)}><Copy className="h-4 w-4" /></Button></span></div>
                  </> : null}
                  <div className="flex items-center justify-between gap-3 py-3"><span className="text-slate-500">{t("marketBilling.binance.remaining")}</span><strong className="tabular-nums">{countdown}</strong></div>
                  <div className="flex items-center justify-between gap-3 py-3"><span className="text-slate-500">{t("marketBilling.binance.statusLabel")}</span><strong>{statusLabel(intent.status, t)}</strong></div>
                </div>
                {canTransfer ? <p className="rounded-md border border-amber-200 bg-amber-50 px-3 py-2 text-xs leading-5 text-amber-950">{t("marketBilling.binance.path")}</p> : null}
                <div className="flex justify-center gap-2">
                  {["pending", "expired", "cancelled"].includes(intent.status) ? <Button size="sm" variant="outline" isDisabled={!!busy || intent.accountStatus !== "verified"} onClick={() => void refreshIntent()}>{busy === "refresh" ? <Loader2 className="h-4 w-4 animate-spin" /> : <RefreshCw className="h-4 w-4" />}{t("marketBilling.binance.refresh")}</Button> : null}
                  {intent.status === "pending" ? <Button size="sm" variant="outline" isDisabled={!!busy} onClick={() => void cancelIntent()}>{busy === "cancel" ? <Loader2 className="h-4 w-4 animate-spin" /> : <Ban className="h-4 w-4" />}{t("marketBilling.binance.cancel")}</Button> : null}
                </div>
              </>
            ) : null}
          </Modal.Body>
          <Modal.Footer>
            <Button variant="ghost" isDisabled={!!busy} onClick={onClose}>{intent ? t("common.close") : t("common.cancel")}</Button>
            {!intent ? <Button variant="primary" isDisabled={!!busy || amountMinor == null} onClick={() => void createIntent()}>{busy === "create" ? <Loader2 className="h-4 w-4 animate-spin" /> : null}{t("marketBilling.prepaid.topup.create")}</Button> : null}
          </Modal.Footer>
        </Modal.Dialog>
      </Modal.Container>
    </Modal.Backdrop>
  );
}
