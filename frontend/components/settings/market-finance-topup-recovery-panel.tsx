"use client";

import * as React from "react";
import { Alert, Button, Modal } from "@heroui/react";
import { Loader2, RefreshCw } from "lucide-react";
import { ConfirmAlertDialog } from "@/components/common/confirm-alert-dialog";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { ApiError } from "@/lib/api";
import {
  getMarketFinanceTopups,
  getMarketFinanceTopupObservations,
  cancelMarketFinanceTopup,
  recoverMarketFinanceTopup,
  type MarketFinanceTopup,
  type MarketFinanceTopupRecovery,
  type MarketTopupGatewayObservation,
} from "@/lib/market-wallet-api";

function dateText(value: string, locale: string) {
  const date = new Date(value);
  return Number.isNaN(date.getTime())
    ? value
    : new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short" }).format(date);
}

export function MarketFinanceTopupRecoveryPanel({ onRecovered }: { onRecovered: () => Promise<unknown> }) {
  const { locale, t } = useLocaleText();
  const [topups, setTopups] = React.useState<MarketFinanceTopup[]>([]);
  const [nextCursor, setNextCursor] = React.useState<string | null>(null);
  const [loadingMore, setLoadingMore] = React.useState(false);
  const listVersion = React.useRef(0);
  const [selectedId, setSelectedId] = React.useState<string | null>(null);
  const [gatewayId, setGatewayId] = React.useState("");
  const [manualReference, setManualReference] = React.useState("");
  const [manualChecked, setManualChecked] = React.useState(false);
  const [confirmOpen, setConfirmOpen] = React.useState(false);
  const [cancelConfirmOpen, setCancelConfirmOpen] = React.useState(false);
  const [preview, setPreview] = React.useState<MarketFinanceTopupRecovery | null>(null);
  const [observations, setObservations] = React.useState<MarketTopupGatewayObservation[]>([]);
  const [observationCursor, setObservationCursor] = React.useState<string | null>(null);
  const [observationsLoading, setObservationsLoading] = React.useState(false);
  const observationVersion = React.useRef(0);
  const [loading, setLoading] = React.useState(true);
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState("");
  const [notice, setNotice] = React.useState("");
  const selected = topups.find((item) => item.id === selectedId) ?? null;
  const manualReferenceValid = /^[A-Za-z0-9_.:/-]{8,128}$/.test(manualReference.trim());
  const canSubmit = !!preview?.canRecover && (!preview.requiresManualReference || (manualReferenceValid && manualChecked));
  const previewDeadline = preview?.gatewayStatus === "awaiting_payment" ? preview.selectionDeadline : preview?.expiresAt;
  const statusLabel = (status: string) => t(`marketWallet.status.${status}` as Parameters<typeof t>[0]);
  const blockerLabel = (code: string) => {
    const known = new Set([
      "gateway_not_terminal", "gateway_paid_source", "gateway_transaction", "gateway_paid_at",
      "gateway_confirmations", "gateway_settled_amount", "gateway_identity_mismatch",
      "local_receipt_present", "local_journal_present", "payment_or_unknown_event",
      "unprocessed_or_disputed_event", "unclassified_review_case", "verification_payment_evidence",
      "verification_identity_or_create_conflict", "other_review_case", "review_cause_unclassified",
    ]);
    return known.has(code)
      ? t(`settings.marketFinance.topupRecovery.blocker.${code}` as Parameters<typeof t>[0])
      : code;
  };

  const reload = React.useCallback(async () => {
    const version = ++listVersion.current;
    try {
      const page = await getMarketFinanceTopups();
      if (version !== listVersion.current) return;
      setTopups(page.items);
      setNextCursor(page.nextCursor);
      setPreview(null);
    } finally {
      if (version === listVersion.current) setLoading(false);
    }
  }, []);

  React.useEffect(() => {
    let active = true;
    const version = ++listVersion.current;
    getMarketFinanceTopups().then((page) => {
      if (active && version === listVersion.current) {
        setTopups(page.items);
        setNextCursor(page.nextCursor);
      }
    }).catch((cause) => {
      if (active && version === listVersion.current) setError(cause instanceof Error ? cause.message : String(cause));
    }).finally(() => {
      if (active && version === listVersion.current) setLoading(false);
    });
    return () => { active = false; };
  }, []);

  const loadMore = async () => {
    if (!nextCursor || loadingMore) return;
    const version = listVersion.current;
    setLoadingMore(true);
    setError("");
    try {
      const page = await getMarketFinanceTopups(nextCursor);
      if (version !== listVersion.current) return;
      setTopups((current) => {
        const seen = new Set(current.map((item) => item.id));
        return [...current, ...page.items.filter((item) => !seen.has(item.id))];
      });
      setNextCursor(page.nextCursor);
    } catch (cause) {
      if (version === listVersion.current) setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setLoadingMore(false);
    }
  };

  const select = (item: MarketFinanceTopup) => {
    setSelectedId(item.id);
    setGatewayId(item.gatewayOrderId ?? "");
    setManualReference("");
    setManualChecked(false);
    setPreview(null);
    setError("");
    setNotice("");
    void loadObservations(item.id);
  };

  const loadObservations = async (id: string, cursor?: string) => {
    const version = ++observationVersion.current;
    if (!cursor) {
      setObservations([]);
      setObservationCursor(null);
    }
    setObservationsLoading(true);
    try {
      const page = await getMarketFinanceTopupObservations(id, cursor);
      if (version === observationVersion.current) {
        setObservations((current) => cursor ? [...current, ...page.items] : page.items);
        setObservationCursor(page.nextCursor);
      }
    } catch (cause) {
      if (version === observationVersion.current) setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      if (version === observationVersion.current) setObservationsLoading(false);
    }
  };

  const run = async (dryRun: boolean) => {
    if (!selected || busy) return;
    setBusy(true);
    setError("");
    try {
      const result = await recoverMarketFinanceTopup(selected.id, {
        expectedStatus: selected.status,
        expectedUpdatedAt: selected.updatedAt,
        gatewayOrderId: gatewayId.trim() || undefined,
        reasonCode: preview?.requiresManualReference && !dryRun
          ? "historical_merchant_review"
          : "gateway_terminal_verified",
        manualReference: !dryRun && preview?.requiresManualReference ? manualReference.trim() : undefined,
        manualConfirmed: !dryRun && !!preview?.requiresManualReference && manualChecked,
        dryRun,
      });
      if (dryRun) {
        setPreview(result);
        void loadObservations(selected.id);
      } else {
        setConfirmOpen(false);
        setPreview(null);
        setSelectedId(null);
        try {
          await Promise.all([reload(), onRecovered()]);
        } catch {
          setError(t("settings.marketFinance.topupRecovery.closedRefreshFailed"));
        }
      }
    } catch (cause) {
      setConfirmOpen(false);
      setPreview(null);
      setError(cause instanceof ApiError && cause.status === 409
        ? t("settings.marketFinance.topupRecovery.conflict")
        : cause instanceof Error ? cause.message : String(cause));
      await reload().catch(() => {});
    } finally {
      setBusy(false);
    }
  };

  const cancelSelected = async () => {
    if (!selected || !preview?.canCancel || busy) return;
    setBusy(true);
    setError("");
    setNotice("");
    try {
      const result = await cancelMarketFinanceTopup(selected.id);
      setCancelConfirmOpen(false);
      setPreview(null);
      setNotice(t(result.status === "review_required"
        ? "settings.marketFinance.topupRecovery.cancelReviewHeld"
        : result.status === "expired"
          ? "settings.marketFinance.topupRecovery.expiredDone"
          : "settings.marketFinance.topupRecovery.cancelDone"));
      try {
        await Promise.all([reload(), onRecovered()]);
      } catch {
        setError(t("settings.marketFinance.topupRecovery.closedRefreshFailed"));
      }
    } catch (cause) {
      setCancelConfirmOpen(false);
      setPreview(null);
      setError(cause instanceof ApiError && cause.status === 409
        ? t("settings.marketFinance.topupRecovery.conflict")
        : cause instanceof Error ? cause.message : String(cause));
      await reload().catch(() => {});
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="grid min-w-0 gap-3" aria-label={t("settings.marketFinance.topupRecovery.title")}>
      <div className="flex items-center justify-between gap-3">
        <div>
          <h4 className="text-sm font-semibold">{t("settings.marketFinance.topupRecovery.title")}</h4>
          <p className="text-xs text-muted-foreground">{t("settings.marketFinance.topupRecovery.note")}</p>
        </div>
        <Button size="sm" variant="ghost" isDisabled={busy || loadingMore} onPress={() => { setError(""); void reload().catch((cause) => setError(cause instanceof Error ? cause.message : String(cause))); }}>
          <RefreshCw className="size-4" aria-hidden="true" />{t("common.reload")}
        </Button>
      </div>
      {loading ? <p className="flex items-center gap-2 text-sm text-muted-foreground"><Loader2 className="size-4 animate-spin" />{t("common.loading")}</p> : null}
      {error ? <Alert status="danger" role="alert">{error}</Alert> : null}
      {notice ? <Alert status="success" role="status">{notice}</Alert> : null}
      {!loading && !topups.length ? <p className="text-sm text-muted-foreground">{t("settings.marketFinance.topupRecovery.empty")}</p> : null}
      {topups.length ? <ul className="grid gap-2">
        {topups.map((item) => <li key={item.id} className="flex min-w-0 flex-wrap items-center justify-between gap-2 rounded-md border border-border px-3 py-2 text-sm">
          <div className="min-w-0">
            <p className="font-medium">${(item.amountMinor / 100).toFixed(2)} · {statusLabel(item.status)}{item.buyerAbandonedAt ? ` · ${t("settings.marketFinance.topupRecovery.buyerAbandoned")}` : ""}</p>
            <p className="break-all text-xs text-muted-foreground">{t("settings.marketFinance.topupRecovery.user")}: {item.userId}</p>
          </div>
          <Button size="sm" variant="outline" isDisabled={busy} onPress={() => select(item)}>
            {t("settings.marketFinance.topupRecovery.inspect")}
          </Button>
        </li>)}
      </ul> : null}
      {nextCursor ? <Button size="sm" variant="outline" isDisabled={busy || loadingMore} onPress={() => void loadMore()}>
        {loadingMore ? <Loader2 className="size-4 animate-spin" aria-hidden="true" /> : null}
        {t("settings.marketFinance.topupRecovery.loadMore")}
      </Button> : null}
      {selected ? <div className="grid gap-3 rounded-md bg-muted/50 p-3">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <p className="text-sm font-semibold">${(selected.amountMinor / 100).toFixed(2)} · {statusLabel(selected.status)}{selected.buyerAbandonedAt ? ` · ${t("settings.marketFinance.topupRecovery.buyerAbandoned")}` : ""}</p>
          <Button size="sm" variant="ghost" isDisabled={busy} onPress={() => { observationVersion.current += 1; setSelectedId(null); setPreview(null); setObservationCursor(null); }}>
            {t("settings.marketFinance.topupRecovery.close")}
          </Button>
        </div>
        <details className="text-xs text-muted-foreground">
          <summary className="cursor-pointer">{t("settings.marketFinance.topupRecovery.identifiers")}</summary>
          <p className="mt-2 break-all font-mono">Router: {selected.id}</p>
          <p className="break-all font-mono">Merchant: {selected.merchantOrderId}</p>
        </details>
        <div className="grid gap-1 text-xs text-muted-foreground">
          <p className="font-medium text-foreground">{t("settings.marketFinance.topupRecovery.observations")}</p>
          {observations.length ? observations.map((observation) => (
            <p key={observation.id} className="break-words">
              {observation.observationCount > 1
                ? t("settings.marketFinance.topupRecovery.observedRange", { first: dateText(observation.firstObservedAt, locale), last: dateText(observation.lastObservedAt, locale) })
                : dateText(observation.firstObservedAt, locale)} · {t(`settings.marketFinance.topupRecovery.source.${observation.source}` as Parameters<typeof t>[0])} · {observation.gatewayStatus} · {observation.evidenceFlags.length
                ? observation.evidenceFlags.map(blockerLabel).join(" · ")
                : t("settings.marketFinance.topupRecovery.noPaymentMarkers")}
              {observation.observationCount > 1 ? ` · ×${observation.observationCount}` : ""}
            </p>
          )) : !observationsLoading ? <p>{t("settings.marketFinance.topupRecovery.noObservations")}</p> : null}
          {observationsLoading ? <p>{t("common.loading")}</p> : null}
          {observationCursor ? <Button size="sm" variant="ghost" isDisabled={observationsLoading || busy} onPress={() => void loadObservations(selected.id, observationCursor)}>
            {t("settings.marketFinance.topupRecovery.moreObservations")}
          </Button> : null}
        </div>
        <label className="grid gap-1 text-xs font-medium">
          {t("settings.marketFinance.topupRecovery.gatewayId")}
          <input className="h-11 min-w-0 rounded-md border border-border bg-background px-3 font-mono text-sm"
            value={gatewayId} readOnly={!!selected.gatewayOrderId}
            onChange={(event) => { setGatewayId(event.target.value); setPreview(null); }} />
        </label>
        {preview ? <div className="grid gap-2" role="status">
          <Alert status={preview.canRecover ? "success" : "warning"}>
            {t(preview.canRecover ? "settings.marketFinance.topupRecovery.ready" : "settings.marketFinance.topupRecovery.blocked", { status: preview.gatewayStatus })}
          </Alert>
          {preview.gatewayStatus === "awaiting_payment" ? <p className="text-xs text-muted-foreground">{t("settings.marketFinance.topupRecovery.awaitingMethod")}</p> : null}
          {previewDeadline ? <p className="text-xs text-muted-foreground">{t("settings.marketFinance.topupRecovery.gatewayDeadline", { time: dateText(previewDeadline, locale) })}</p> : null}
          {preview.blockers.length ? <ul className="list-disc pl-5 text-xs text-muted-foreground">
            {preview.blockers.map((code) => <li key={code}>{blockerLabel(code)}</li>)}
          </ul> : null}
          {preview.reviewCaseCodes.length ? <p className="text-xs text-muted-foreground">
            {t("settings.marketFinance.topupRecovery.cases")}: {preview.reviewCaseCodes.map((code) => {
              if (code === "unknown_gateway_event") return t("settings.marketFinance.topupRecovery.case.unknown_gateway_event");
              if (code === "topup_verification_failed") return t("settings.marketFinance.topupRecovery.case.topup_verification_failed");
              return code;
            }).join(" · ")}
          </p> : null}
        </div> : null}
        {preview?.canRecover && preview.requiresManualReference ? <div className="grid gap-2 rounded-md border border-amber-300 bg-amber-50 p-3 text-xs text-amber-950">
          <p>{t("settings.marketFinance.topupRecovery.manualNote")}</p>
          <label className="grid gap-1 font-medium">
            {t("settings.marketFinance.topupRecovery.manualReference")}
            <input className="h-11 min-w-0 rounded-md border border-amber-400 bg-background px-3 font-mono text-sm text-foreground"
              value={manualReference} maxLength={128} onChange={(event) => { setManualReference(event.target.value); setManualChecked(false); }} />
          </label>
          <label className="flex items-start gap-2">
            <input type="checkbox" checked={manualChecked} onChange={(event) => setManualChecked(event.target.checked)} />
            {t("settings.marketFinance.topupRecovery.manualConfirm")}
          </label>
        </div> : null}
        <div className="flex flex-wrap gap-2">
          <Button size="sm" variant="outline" isDisabled={busy || !gatewayId.trim()} onPress={() => void run(true)}>
            {busy ? <Loader2 className="size-4 animate-spin" aria-hidden="true" /> : null}
            {t("settings.marketFinance.topupRecovery.verify")}
          </Button>
          <Button size="sm" isDisabled={busy || !canSubmit} onPress={() => setConfirmOpen(true)}>
            {t("settings.marketFinance.topupRecovery.recover")}
          </Button>
          {preview?.canCancel ? <Button size="sm" variant="outline" isDisabled={busy} onPress={() => setCancelConfirmOpen(true)}>
            {t("settings.marketFinance.topupRecovery.cancelGateway")}
          </Button> : null}
        </div>
      </div> : null}
      <Modal.Backdrop isOpen={confirmOpen} onOpenChange={(open) => { if (!busy) setConfirmOpen(open); }}>
        <Modal.Container>
          <Modal.Dialog>
            <Modal.Header><Modal.Heading>{t("settings.marketFinance.topupRecovery.confirmTitle")}</Modal.Heading></Modal.Header>
            <Modal.Body className="grid gap-2 text-sm">
              <p>{t("settings.marketFinance.topupRecovery.confirmNote", { amount: selected ? `$${(selected.amountMinor / 100).toFixed(2)}` : "" })}</p>
              <p className="break-all font-mono text-xs text-muted-foreground">{selected?.merchantOrderId}</p>
            </Modal.Body>
            <Modal.Footer>
              <Button variant="outline" isDisabled={busy} onPress={() => setConfirmOpen(false)}>{t("common.cancel")}</Button>
              <Button isDisabled={busy || !canSubmit} onPress={() => void run(false)}>
                {busy ? <Loader2 className="size-4 animate-spin" aria-hidden="true" /> : null}
                {t("settings.marketFinance.topupRecovery.recover")}
              </Button>
            </Modal.Footer>
          </Modal.Dialog>
        </Modal.Container>
      </Modal.Backdrop>
      <ConfirmAlertDialog
        open={cancelConfirmOpen}
        title={t("settings.marketFinance.topupRecovery.cancelTitle")}
        description={t("settings.marketFinance.topupRecovery.cancelNote")}
        confirmLabel={t("settings.marketFinance.topupRecovery.cancelGateway")}
        cancelLabel={t("common.cancel")}
        tone="warning"
        busy={busy}
        onConfirm={() => void cancelSelected()}
        onOpenChange={(open) => { if (!busy) setCancelConfirmOpen(open); }}
      />
    </section>
  );
}
