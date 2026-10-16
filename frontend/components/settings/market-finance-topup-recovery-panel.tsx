"use client";

import * as React from "react";
import { Alert, Button } from "@heroui/react";
import { Loader2, RefreshCw } from "lucide-react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { ApiError } from "@/lib/api";
import {
  getMarketFinanceTopups,
  recoverMarketFinanceTopup,
  type MarketFinanceTopup,
  type MarketFinanceTopupRecovery,
} from "@/lib/market-wallet-api";

export function MarketFinanceTopupRecoveryPanel({ onRecovered }: { onRecovered: () => Promise<unknown> }) {
  const { t } = useLocaleText();
  const [topups, setTopups] = React.useState<MarketFinanceTopup[]>([]);
  const [selectedId, setSelectedId] = React.useState<string | null>(null);
  const [gatewayId, setGatewayId] = React.useState("");
  const [reasonCode, setReasonCode] = React.useState("gateway_terminal_verified");
  const [preview, setPreview] = React.useState<MarketFinanceTopupRecovery | null>(null);
  const [loading, setLoading] = React.useState(true);
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState("");
  const recoverableCandidates = topups.filter((item) => item.status === "review_required" || item.status === "create_unknown");
  const selected = recoverableCandidates.find((item) => item.id === selectedId) ?? null;

  const reload = React.useCallback(async () => {
    setTopups(await getMarketFinanceTopups());
    setPreview(null);
  }, []);

  React.useEffect(() => {
    let active = true;
    getMarketFinanceTopups().then((items) => {
      if (active) setTopups(items);
    }).catch((cause) => {
      if (active) setError(cause instanceof Error ? cause.message : String(cause));
    }).finally(() => {
      if (active) setLoading(false);
    });
    return () => { active = false; };
  }, []);

  const select = (item: MarketFinanceTopup) => {
    setSelectedId(item.id);
    setGatewayId(item.gatewayOrderId ?? "");
    setPreview(null);
    setError("");
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
        reasonCode: reasonCode.trim(),
        dryRun,
      });
      if (dryRun) {
        setPreview(result);
      } else {
        setPreview(null);
        setSelectedId(null);
        await Promise.all([reload(), onRecovered()]);
      }
    } catch (cause) {
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
        <Button size="sm" variant="ghost" isDisabled={busy} onPress={() => void reload().catch((cause) => setError(cause instanceof Error ? cause.message : String(cause)))}>
          <RefreshCw className="size-4" aria-hidden="true" />{t("common.reload")}
        </Button>
      </div>
      {loading ? <p className="flex items-center gap-2 text-sm text-muted-foreground"><Loader2 className="size-4 animate-spin" />{t("common.loading")}</p> : null}
      {error ? <Alert status="danger" role="alert">{error}</Alert> : null}
      {!loading && !recoverableCandidates.length ? <p className="text-sm text-muted-foreground">{t("settings.marketFinance.topupRecovery.empty")}</p> : null}
      {recoverableCandidates.length ? <ul className="grid gap-2">
        {recoverableCandidates.map((item) => <li key={item.id} className="flex min-w-0 flex-wrap items-center justify-between gap-2 rounded-md border border-border px-3 py-2 text-sm">
          <div className="min-w-0">
            <p className="font-medium">${(item.amountMinor / 100).toFixed(2)} · {item.status}</p>
            <p className="break-all font-mono text-xs text-muted-foreground">{item.id} · {item.userId}</p>
          </div>
          <Button size="sm" variant="outline" isDisabled={busy} onPress={() => select(item)}>
            {t("settings.marketFinance.topupRecovery.inspect")}
          </Button>
        </li>)}
      </ul> : null}
      {selected ? <div className="grid gap-3 rounded-md bg-muted/50 p-3">
        <p className="break-all font-mono text-xs">{selected.merchantOrderId}</p>
        <label className="grid gap-1 text-xs font-medium">
          {t("settings.marketFinance.topupRecovery.gatewayId")}
          <input className="h-11 min-w-0 rounded-md border border-border bg-background px-3 font-mono text-sm"
            value={gatewayId} readOnly={!!selected.gatewayOrderId}
            onChange={(event) => { setGatewayId(event.target.value); setPreview(null); }} />
        </label>
        <label className="grid gap-1 text-xs font-medium">
          {t("settings.marketFinance.topupRecovery.reason")}
          <input className="h-11 min-w-0 rounded-md border border-border bg-background px-3 font-mono text-sm"
            value={reasonCode} maxLength={64}
            onChange={(event) => { setReasonCode(event.target.value); setPreview(null); }} />
        </label>
        {preview ? <Alert status={preview.canRecover ? "success" : "warning"} role="status">
          {t(preview.canRecover ? "settings.marketFinance.topupRecovery.ready" : "settings.marketFinance.topupRecovery.blocked", { status: preview.gatewayStatus })}
        </Alert> : null}
        <div className="flex flex-wrap gap-2">
          <Button size="sm" variant="outline" isDisabled={busy || !gatewayId.trim() || reasonCode.trim().length < 3} onPress={() => void run(true)}>
            {busy ? <Loader2 className="size-4 animate-spin" aria-hidden="true" /> : null}
            {t("settings.marketFinance.topupRecovery.verify")}
          </Button>
          <Button size="sm" isDisabled={busy || !preview?.canRecover} onPress={() => void run(false)}>
            {t("settings.marketFinance.topupRecovery.recover")}
          </Button>
        </div>
      </div> : null}
    </section>
  );
}
