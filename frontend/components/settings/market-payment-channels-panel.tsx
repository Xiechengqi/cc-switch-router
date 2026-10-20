"use client";

/* Hallmark · pre-emit critique: P4 H4 E4 S5 R4 V4 */
/* Hallmark · component: finance settings panel · genre: modern-minimal · theme: existing Router system
 * states: default · hover · focus · active · disabled · loading · error · success
 * contrast: inherited Router tokens
 */

import * as React from "react";
import { AlertTriangle, CheckCircle2, Loader2, Plus, RefreshCw } from "lucide-react";
import { Alert, Button, Modal } from "@heroui/react";
import { CopyableCodeField } from "@/components/common/copyable-code-field";
import { MarketFinanceTransfersPanel } from "@/components/settings/market-finance-transfers-panel";
import { MarketFinanceDisputesPanel } from "@/components/settings/market-finance-disputes-panel";
import { MarketFinanceTopupRecoveryPanel } from "@/components/settings/market-finance-topup-recovery-panel";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { ApiError } from "@/lib/api";
import {
  createMarketPaymentChannel,
  getMarketFinanceCases,
  getMarketFinanceReadiness,
  getMarketPaymentChannels,
  setMarketFinanceMode,
  setMarketFinancePause,
  setMarketPaymentChannelEnabled,
  setMarketPaymentChannelArchived,
  updateMarketPaymentChannelDetails,
  verifyMarketPaymentChannelApi,
  rotateMarketPaymentChannelApi,
  rotateMarketPaymentChannelWebhook,
  type MarketFinanceCase,
  type MarketFinanceReadiness,
  type MarketPaymentChannel,
} from "@/lib/market-wallet-api";

type FormState = {
  displayName: string;
  sortOrder: string;
  apiKeyId: string;
  apiSecret: string;
  webhookSecret: string;
  webhookMode: "signed" | "poll_only";
  emergencyRevokePrevious: boolean;
};

type DialogState = { kind: "create" | "details" | "api" | "webhook" | "archive"; channel?: MarketPaymentChannel };

const EMPTY_FORM: FormState = {
  displayName: "DujiaoPay",
  sortOrder: "0",
  apiKeyId: "",
  apiSecret: "",
  webhookSecret: "",
  webhookMode: "signed",
  emergencyRevokePrevious: false,
};

function message(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

function channelBlockerKey(code: string) {
  switch (code) {
    case "credited_topup_missing_evidence": return "settings.marketFinance.blocker.credited_topup_missing_evidence" as const;
    case "channel_none_enabled": return "settings.marketFinance.blocker.channel_none_enabled" as const;
    case "channel_api_unverified": return "settings.marketFinance.blocker.channel_api_unverified" as const;
    case "channel_not_signed": return "settings.marketFinance.blocker.channel_not_signed" as const;
    case "channel_webhook_unverified": return "settings.marketFinance.blocker.channel_webhook_unverified" as const;
    case "channel_no_current_live_topup": return "settings.marketFinance.blocker.channel_no_current_live_topup" as const;
    default: return null;
  }
}

export function MarketPaymentChannelsPanel() {
  const { t } = useLocaleText();
  const [readiness, setReadiness] = React.useState<MarketFinanceReadiness | null>(null);
  const [channels, setChannels] = React.useState<MarketPaymentChannel[]>([]);
  const [cases, setCases] = React.useState<MarketFinanceCase[]>([]);
  const [casesNextCursor, setCasesNextCursor] = React.useState<string | null>(null);
  const [casesLoadingMore, setCasesLoadingMore] = React.useState(false);
  const casesListVersion = React.useRef(0);
  const [form, setForm] = React.useState<FormState>(EMPTY_FORM);
  const [dialog, setDialog] = React.useState<DialogState | null>(null);
  const [showArchived, setShowArchived] = React.useState(false);
  const [manageChannelId, setManageChannelId] = React.useState<string | null>(null);
  const [busy, setBusy] = React.useState("");
  const [error, setError] = React.useState("");
  const [loaded, setLoaded] = React.useState(false);
  const busyRef = React.useRef(false);

  const reload = React.useCallback(async () => {
    const casesVersion = ++casesListVersion.current;
    const [nextReadiness, nextChannels, nextCases] = await Promise.all([
      getMarketFinanceReadiness(),
      getMarketPaymentChannels(),
      getMarketFinanceCases(),
    ]);
    if (casesVersion !== casesListVersion.current) return;
    setReadiness(nextReadiness);
    setChannels(nextChannels);
    setCases(nextCases.items);
    setCasesNextCursor(nextCases.nextCursor);
    setLoaded(true);
  }, []);

  React.useEffect(() => {
    let active = true;
    const casesVersion = ++casesListVersion.current;
    Promise.all([
      getMarketFinanceReadiness(),
      getMarketPaymentChannels(),
      getMarketFinanceCases(),
    ]).then(([nextReadiness, nextChannels, nextCases]) => {
      if (!active || casesVersion !== casesListVersion.current) return;
      setReadiness(nextReadiness);
      setChannels(nextChannels);
      setCases(nextCases.items);
      setCasesNextCursor(nextCases.nextCursor);
      setLoaded(true);
    }).catch((cause) => {
      if (!active) return;
      setError(message(cause));
      setLoaded(true);
    });
    return () => { active = false; };
  }, []);

  const loadMoreCases = async () => {
    if (!casesNextCursor || casesLoadingMore) return;
    const version = casesListVersion.current;
    setCasesLoadingMore(true);
    setError("");
    try {
      const page = await getMarketFinanceCases(casesNextCursor);
      if (version !== casesListVersion.current) return;
      setCases((current) => {
        const seen = new Set(current.map((item) => item.id));
        return [...current, ...page.items.filter((item) => !seen.has(item.id))];
      });
      setCasesNextCursor(page.nextCursor);
    } catch (cause) {
      if (version === casesListVersion.current) setError(message(cause));
    } finally {
      setCasesLoadingMore(false);
    }
  };

  const perform = async (name: string, action: () => Promise<unknown>) => {
    if (busyRef.current) return;
    busyRef.current = true;
    setBusy(name);
    setError("");
    try {
      await action();
      await reload();
    } catch (cause) {
      if (cause instanceof ApiError && cause.status === 409 && cause.message.includes("version changed")) {
        await reload().catch(() => {});
        setDialog(null);
        setForm(EMPTY_FORM);
        setError(t("settings.marketFinance.versionChanged"));
      } else if (cause instanceof ApiError && cause.status === 409 && cause.message.includes("has not passed verification")) {
        setError(t("settings.marketFinance.channelNeedsVerification"));
      } else {
        setError(message(cause));
      }
    } finally {
      busyRef.current = false;
      setBusy("");
    }
  };

  const openDialog = (kind: DialogState["kind"], channel?: MarketPaymentChannel) => {
    setError("");
    setDialog({ kind, channel });
    setForm({
      ...EMPTY_FORM,
      displayName: channel?.displayName ?? "DujiaoPay",
      sortOrder: String(channel?.sortOrder ?? channels.length),
      webhookMode: kind === "webhook" ? "signed" : channel?.webhookMode ?? "signed",
    });
  };

  const closeDialog = () => {
    setDialog(null);
    setForm(EMPTY_FORM);
    setError("");
  };

  const submitDialog = async (event: React.FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (!dialog) return;
    const channel = dialog.channel;
    await perform(dialog.kind, async () => {
      if (dialog.kind === "create") {
        await createMarketPaymentChannel({
          displayName: form.displayName.trim(), sortOrder: Number(form.sortOrder),
          apiKeyId: form.apiKeyId.trim(), apiSecret: form.apiSecret,
          webhookSecret: form.webhookMode === "signed" ? form.webhookSecret : undefined,
          webhookMode: form.webhookMode, allowedMethods: [],
        });
      } else if (channel && dialog.kind === "details") {
        await updateMarketPaymentChannelDetails(channel.id, {
          displayName: form.displayName.trim(), sortOrder: Number(form.sortOrder), expectedVersion: channel.version,
        });
      } else if (channel && dialog.kind === "api") {
        await rotateMarketPaymentChannelApi(channel.id, {
          apiKeyId: form.apiKeyId.trim(), apiSecret: form.apiSecret, expectedVersion: channel.version,
        });
      } else if (channel && dialog.kind === "webhook") {
        await rotateMarketPaymentChannelWebhook(channel.id, {
          webhookMode: form.webhookMode,
          webhookSecret: form.webhookMode === "signed" ? form.webhookSecret : undefined,
          emergencyRevokePrevious: form.emergencyRevokePrevious,
          expectedVersion: channel.version,
        });
      } else if (channel && dialog.kind === "archive") {
        await setMarketPaymentChannelArchived(channel.id, !channel.archivedAt, channel.version);
      }
      closeDialog();
    });
  };

  const activeChannels = channels.filter((channel) => !channel.archivedAt);
  const archivedChannels = channels.filter((channel) => !!channel.archivedAt);
  const dialogValid = !!dialog && (dialog.kind === "archive" || (
    (dialog.kind !== "create" && dialog.kind !== "details" || (!!form.displayName.trim() && Number.isSafeInteger(Number(form.sortOrder))))
    && (dialog.kind !== "create" && dialog.kind !== "api" || (!!form.apiKeyId.trim() && form.apiSecret.length >= 16))
    && (dialog.kind !== "create" && dialog.kind !== "webhook" || form.webhookMode === "poll_only" || form.webhookSecret.length >= 16)
    && !(dialog.kind === "webhook" && dialog.channel?.webhookMode === "poll_only" && form.webhookMode === "poll_only")
  ));

  return (
    <section className="grid min-w-0 gap-6" aria-label={t("settings.marketFinance.title")}>
      <div className="flex min-w-0 flex-wrap items-start justify-between gap-3">
        <div className="min-w-0">
          <h3 className="text-lg font-semibold">{t("settings.marketFinance.title")}</h3>
          <p className="mt-1 max-w-2xl text-sm text-muted-foreground">
            {t("settings.marketFinance.description")}
          </p>
        </div>
        <Button
          variant="ghost"
          size="sm"
          aria-label={t("settings.marketFinance.refresh")}
          isDisabled={!!busy}
          onPress={() => void perform("refresh", reload)}
        >
          <RefreshCw className="size-4" /> {t("settings.marketFinance.refresh")}
        </Button>
      </div>

      {!loaded ? <p className="flex items-center gap-2 text-sm text-muted-foreground"><Loader2 className="size-4 animate-spin" />{t("common.loading")}</p> : null}
      {error && !dialog ? <Alert status="danger" role="alert">{error}</Alert> : null}

      {readiness ? (
        <div className="grid min-w-0 gap-4 rounded-lg border border-border bg-card p-4 sm:p-5">
          <div className="flex flex-wrap items-center justify-between gap-3">
            <div className="flex items-center gap-2">
              <span className="text-sm font-semibold">{t("settings.marketFinance.mode")}</span>
              <span className="rounded-full bg-muted px-2.5 py-1 text-xs font-medium">
                {t(`settings.marketFinance.mode.${readiness.mode}`)}
              </span>
            </div>
            <div className="flex flex-wrap gap-2">
              {readiness.mode === "shadow" ? (
                <Button size="sm" variant="outline" isDisabled={!!busy} onPress={() => void perform("mode", () => setMarketFinanceMode("disabled", readiness.version))}>
                  {t("settings.marketFinance.disable")}
                </Button>
              ) : null}
              {readiness.mode === "disabled" ? (
                <Button size="sm" variant="outline" isDisabled={!!busy} onPress={() => void perform("mode", () => setMarketFinanceMode("shadow", readiness.version))}>
                  {t("settings.marketFinance.shadow")}
                </Button>
              ) : null}
              {readiness.mode === "shadow" ? (
                <Button size="sm" isDisabled={!!busy || !readiness.canEnable} onPress={() => void perform("mode", () => setMarketFinanceMode("enabled", readiness.version))}>
                  {t("settings.marketFinance.enable")}
                </Button>
              ) : null}
            </div>
          </div>
          <div className="grid gap-1 text-sm text-muted-foreground sm:grid-cols-2">
            <p>{t("settings.marketFinance.verifiedTopups", { count: readiness.verifiedLiveTopups })}</p>
            <p>{t("settings.marketFinance.blockers", { count: readiness.legacyBlockers.length })}</p>
            <p>{t("settings.marketFinance.channelBlockers", { count: readiness.paymentChannelBlockers.length })}</p>
            <p>{t("settings.marketFinance.criticalCases", { count: readiness.criticalCases })}</p>
            {readiness.staleTopupReviews > 0 ? <p className="font-medium text-amber-800">{t("settings.marketFinance.staleTopupReviews", { count: readiness.staleTopupReviews })}</p> : null}
          </div>
          {readiness.mode === "enabled" ? (
            <div className="grid gap-3 rounded-md border border-border p-3 sm:grid-cols-2">
              <div className="flex items-center justify-between gap-3">
                <span className="flex flex-wrap items-center gap-2 text-sm">{t("settings.marketFinance.newOrders")}
                  <span className={readiness.newOrdersPaused ? "rounded-full bg-amber-100 px-2 py-0.5 text-xs text-amber-900" : "rounded-full bg-emerald-100 px-2 py-0.5 text-xs text-emerald-900"}>
                    {t(readiness.newOrdersPaused ? "settings.marketFinance.statusPaused" : "settings.marketFinance.statusOpen")}
                  </span>
                </span>
                <Button size="sm" variant="outline" isDisabled={!!busy} onPress={() => void perform("pause-orders", () => setMarketFinancePause(!readiness.newOrdersPaused, readiness.newWithdrawalsPaused, readiness.version))}>
                  {t(readiness.newOrdersPaused ? "settings.marketFinance.resume" : "settings.marketFinance.pause")}
                </Button>
              </div>
              <div className="flex items-center justify-between gap-3">
                <span className="flex flex-wrap items-center gap-2 text-sm">{t("settings.marketFinance.newWithdrawals")}
                  <span className={readiness.newWithdrawalsPaused ? "rounded-full bg-amber-100 px-2 py-0.5 text-xs text-amber-900" : "rounded-full bg-emerald-100 px-2 py-0.5 text-xs text-emerald-900"}>
                    {t(readiness.newWithdrawalsPaused ? "settings.marketFinance.statusPaused" : "settings.marketFinance.statusOpen")}
                  </span>
                </span>
                <Button size="sm" variant="outline" isDisabled={!!busy} onPress={() => void perform("pause-withdrawals", () => setMarketFinancePause(readiness.newOrdersPaused, !readiness.newWithdrawalsPaused, readiness.version))}>
                  {t(readiness.newWithdrawalsPaused ? "settings.marketFinance.resume" : "settings.marketFinance.pause")}
                </Button>
              </div>
              <p className="text-xs text-muted-foreground sm:col-span-2">{t("settings.marketFinance.pauseNote")}</p>
            </div>
          ) : null}
          {readiness.mode !== "enabled" && !readiness.canEnable ? (
            <div className="grid gap-1 text-sm text-muted-foreground">
              <p className="flex items-start gap-2">
              <AlertTriangle className="mt-0.5 size-4 shrink-0" aria-hidden="true" />
              {t("settings.marketFinance.cutoverBlocked")}
              </p>
              {!readiness.codeIntegrated ? <p>{t("settings.marketFinance.codePending")}</p> : null}
              {!readiness.verifiedLiveTopups ? <p>{t("settings.marketFinance.liveTopupMissing")}</p> : null}
            </div>
          ) : null}
          {readiness.legacyBlockers.length ? (
            <ul className="grid gap-1 text-xs text-muted-foreground">
              {readiness.legacyBlockers.map((blocker) => (
                <li key={blocker.code} className="flex justify-between gap-3">
                  <span className="min-w-0 break-all">{blocker.code}</span>
                  <span className="tabular-nums">{blocker.count}</span>
                </li>
              ))}
            </ul>
          ) : null}
          {readiness.paymentChannelBlockers.length ? (
            <ul className="grid gap-1 text-xs text-muted-foreground">
              {readiness.paymentChannelBlockers.map((blocker) => {
                const key = channelBlockerKey(blocker.code);
                return <li key={blocker.code} className="flex justify-between gap-3">
                  <span>{key ? t(key) : blocker.code}</span>
                  <span className="tabular-nums">{blocker.count}</span>
                </li>;
              })}
            </ul>
          ) : null}
        </div>
      ) : null}

      <div className="grid min-w-0 gap-4">
        <div className="flex flex-wrap items-center justify-between gap-3">
          <h4 className="text-sm font-semibold">{t("settings.marketFinance.channels")}</h4>
          <Button size="sm" isDisabled={!!busy} onPress={() => openDialog("create")}>
            <Plus className="size-4" />{t("settings.marketFinance.addChannel")}
          </Button>
        </div>
        {readiness?.webhookUrl ? (
          <div className="grid gap-1">
            <CopyableCodeField
              label={t("settings.marketFinance.webhookUrl")}
              value={readiness.webhookUrl}
              copyLabel={t("clientMarket.copy")}
              copiedLabel={t("clientMarket.copied")}
            />
            <p className="text-xs text-muted-foreground">{t("settings.marketFinance.webhookUrlNote")}</p>
          </div>
        ) : null}
        {activeChannels.length ? (
          <ul className="grid gap-3">
            {activeChannels.map((channel) => (
              <li key={channel.id} className="grid min-w-0 gap-3 rounded-lg border border-border bg-card p-4">
                <div className="flex min-w-0 flex-wrap items-center justify-between gap-2">
                  <p className="min-w-0 truncate text-sm font-semibold">{channel.displayName}</p>
                  <span className="rounded-full bg-muted px-2.5 py-1 text-xs font-medium">
                    {t(channel.enabled ? "settings.marketFinance.channelActive" : "settings.marketFinance.channelStopped")}
                  </span>
                </div>
                <div className="grid gap-1 text-xs text-muted-foreground sm:grid-cols-3">
                  <span className="flex items-center gap-1.5">{channel.verified ? <CheckCircle2 className="size-3.5" /> : <AlertTriangle className="size-3.5" />}{t(channel.verified ? "settings.marketFinance.verified" : "settings.marketFinance.unverified")}</span>
                  <span className="flex items-center gap-1.5">{channel.webhookVerified ? <CheckCircle2 className="size-3.5" /> : <AlertTriangle className="size-3.5" />}{t(channel.webhookMode === "poll_only" ? "settings.marketFinance.webhookMode.poll_only" : channel.webhookVerified ? "settings.marketFinance.webhookVerified" : "settings.marketFinance.webhookPending")}</span>
                  <span>{t("settings.marketFinance.channelCurrentTopups", { count: channel.verifiedLiveTopupsCurrentRevision })}</span>
                </div>
                {channel.unresolvedTopups > 0 ? <p className="text-xs text-muted-foreground">{t("settings.marketFinance.unresolvedTopups", { count: channel.unresolvedTopups })}</p> : null}
                <div className="flex flex-wrap gap-2">
                  <Button size="sm" variant="outline" isDisabled={!!busy || !channel.verified} onPress={() => void perform(channel.id, () => setMarketPaymentChannelEnabled(channel.id, !channel.enabled, channel.version))}>
                    {channel.enabled ? t("settings.marketFinance.stopChannel") : t("settings.marketFinance.startChannel")}
                  </Button>
                  <Button size="sm" variant="ghost" aria-expanded={manageChannelId === channel.id} isDisabled={!!busy} onPress={() => setManageChannelId((current) => current === channel.id ? null : channel.id)}>{t("settings.marketFinance.manageChannel")}</Button>
                </div>
                {manageChannelId === channel.id ? <div className="flex flex-wrap gap-2 rounded-md bg-muted/60 p-2">
                  {!channel.verified ? <Button size="sm" variant="ghost" isDisabled={!!busy} onPress={() => void perform(channel.id, () => verifyMarketPaymentChannelApi(channel.id, channel.version))}>{t("settings.marketFinance.reverifyApi")}</Button> : null}
                  <Button size="sm" variant="ghost" isDisabled={!!busy} onPress={() => openDialog("details", channel)}>{t("settings.marketFinance.editDetails")}</Button>
                  <Button size="sm" variant="ghost" isDisabled={!!busy} onPress={() => openDialog("api", channel)}>{t("settings.marketFinance.rotateApi")}</Button>
                  <Button size="sm" variant="ghost" isDisabled={!!busy} onPress={() => openDialog("webhook", channel)}>{t("settings.marketFinance.rotateWebhook")}</Button>
                  <Button size="sm" variant="ghost" isDisabled={!!busy} onPress={() => openDialog("archive", channel)}>{t("settings.marketFinance.archive")}</Button>
                </div> : null}
              </li>
            ))}
          </ul>
        ) : loaded ? <p className="text-sm text-muted-foreground">{t(channels.length ? "settings.marketFinance.allArchived" : "settings.marketFinance.noChannels")}</p> : null}
        {archivedChannels.length ? (
          <div className="grid gap-2">
            <Button size="sm" variant="ghost" className="justify-self-start" onPress={() => setShowArchived((value) => !value)}>
              {t("settings.marketFinance.archivedChannels", { count: archivedChannels.length })}
            </Button>
            {showArchived ? <ul className="grid gap-2">{archivedChannels.map((channel) => (
              <li key={channel.id} className="flex flex-wrap items-center justify-between gap-2 rounded-lg border border-border bg-card px-4 py-3">
                <div className="min-w-0"><p className="truncate text-sm font-medium">{channel.displayName}</p><p className="text-xs text-muted-foreground">{t("settings.marketFinance.unresolvedTopups", { count: channel.unresolvedTopups })}</p></div>
                <Button size="sm" variant="outline" isDisabled={!!busy} onPress={() => openDialog("archive", channel)}>{t("settings.marketFinance.restore")}</Button>
              </li>
            ))}</ul> : null}
          </div>
        ) : null}
      </div>

      <Modal.Backdrop isOpen={!!dialog} onOpenChange={(open) => { if (!open && !busy) closeDialog(); }}>
        <Modal.Container placement="center">
          <Modal.Dialog className="light w-[min(560px,calc(100vw-2rem))] max-w-none !bg-white !text-slate-900">
            <Modal.CloseTrigger />
            <Modal.Header><Modal.Heading>{dialog ? t(`settings.marketFinance.dialog.${dialog.kind}`) : ""}</Modal.Heading></Modal.Header>
            <form onSubmit={(event) => void submitDialog(event)}>
              <Modal.Body className="grid max-h-[65vh] gap-3 overflow-y-auto">
                {dialog?.kind === "create" || dialog?.kind === "details" ? <>
                  <Field label={t("settings.marketFinance.displayName")} value={form.displayName} onChange={(value) => setForm((current) => ({ ...current, displayName: value }))} maxLength={80} required />
                  <Field label={t("settings.marketFinance.sortOrder")} value={form.sortOrder} onChange={(value) => setForm((current) => ({ ...current, sortOrder: value }))} type="number" required />
                </> : null}
                {dialog?.kind === "create" || dialog?.kind === "api" ? <>
                  <Field label={t("settings.marketFinance.keyId")} value={form.apiKeyId} onChange={(value) => setForm((current) => ({ ...current, apiKeyId: value }))} maxLength={200} required />
                  <Field label={t("settings.marketFinance.apiSecret")} value={form.apiSecret} onChange={(value) => setForm((current) => ({ ...current, apiSecret: value }))} type="password" minLength={16} maxLength={4096} required />
                </> : null}
                {dialog?.kind === "create" || dialog?.kind === "webhook" ? <>
                  <label className="grid gap-1 text-sm font-medium"><span>{t("settings.marketFinance.webhookMode")}</span>
                    <select className="h-11 rounded-md border border-input bg-background px-3 text-sm focus-visible:outline-2 focus-visible:outline-ring" value={form.webhookMode} onChange={(event) => setForm((current) => ({ ...current, webhookMode: event.target.value as FormState["webhookMode"], webhookSecret: "" }))}>
                      <option value="signed">{t("settings.marketFinance.webhookMode.signed")}</option>
                      <option value="poll_only">{t("settings.marketFinance.webhookMode.poll_only")}</option>
                    </select>
                  </label>
                  {form.webhookMode === "signed" ? <Field label={t("settings.marketFinance.webhookSecret")} value={form.webhookSecret} onChange={(value) => setForm((current) => ({ ...current, webhookSecret: value }))} type="password" minLength={16} maxLength={4096} required /> : null}
                  <p className="text-xs text-muted-foreground">{t(form.webhookMode === "signed" ? "settings.marketFinance.signedNote" : "settings.marketFinance.pollOnlyNote")}</p>
                  {dialog.kind === "webhook" && dialog.channel?.webhookMode === "signed" && form.webhookMode === "signed" ? <label className="flex items-start gap-2 text-xs"><input type="checkbox" checked={form.emergencyRevokePrevious} onChange={(event) => setForm((current) => ({ ...current, emergencyRevokePrevious: event.target.checked }))} />{t("settings.marketFinance.emergencyRevoke")}</label> : null}
                </> : null}
                {dialog?.kind === "api" ? <p className="text-xs text-muted-foreground">{t("settings.marketFinance.apiRotationNote")}</p> : null}
                {dialog?.kind === "archive" ? <p className="text-sm text-muted-foreground">{t(dialog.channel?.archivedAt ? "settings.marketFinance.restoreNote" : "settings.marketFinance.archiveNote", { count: dialog.channel?.unresolvedTopups ?? 0 })}</p> : null}
                {error ? <Alert status="danger" role="alert">{error}</Alert> : null}
              </Modal.Body>
              <Modal.Footer>
                <Button type="button" variant="outline" isDisabled={!!busy} onPress={closeDialog}>{t("common.cancel")}</Button>
                <Button type="submit" isDisabled={!!busy || !dialogValid}>
                  {busy ? <Loader2 className="size-4 animate-spin" /> : null}
                  {t(dialog?.kind === "archive" ? dialog.channel?.archivedAt ? "settings.marketFinance.restore" : "settings.marketFinance.archive" : "common.save")}
                </Button>
              </Modal.Footer>
            </form>
          </Modal.Dialog>
        </Modal.Container>
      </Modal.Backdrop>

      {cases.length ? (
        <div className="grid min-w-0 gap-3">
          <h4 className="text-sm font-semibold">{t("settings.marketFinance.cases", { count: cases.length })}</h4>
          <ul className="grid gap-2">
            {cases.map((item) => (
              <li key={item.id} className="flex min-w-0 flex-wrap items-center justify-between gap-2 rounded-md border border-border px-3 py-2 text-sm">
                <span className="min-w-0 break-all font-medium">{item.code}</span>
                <span className="min-w-0 break-all font-mono text-xs text-muted-foreground">{item.objectKind}:{item.objectId}</span>
              </li>
            ))}
          </ul>
          {casesNextCursor ? <Button size="sm" variant="outline" isDisabled={!!busy || casesLoadingMore} onPress={() => void loadMoreCases()}>
            {casesLoadingMore ? <Loader2 className="size-4 animate-spin" aria-hidden="true" /> : null}
            {t("settings.marketFinance.casesLoadMore")}
          </Button> : null}
          <p className="text-xs text-muted-foreground">{t("settings.marketFinance.caseNote")}</p>
        </div>
      ) : null}
      <MarketFinanceTopupRecoveryPanel onRecovered={reload} />
      <MarketFinanceTransfersPanel financeMode={readiness?.mode ?? "disabled"} withdrawalsPaused={readiness?.newWithdrawalsPaused ?? false} />
      <MarketFinanceDisputesPanel />
    </section>
  );
}

function Field({ label, value, onChange, type = "text", minLength, maxLength, required }: {
  label: string;
  value: string;
  onChange: (value: string) => void;
  type?: "text" | "password" | "number";
  minLength?: number;
  maxLength?: number;
  required?: boolean;
}) {
  return (
    <label className="grid min-w-0 gap-1 text-sm font-medium">
      <span>{label}</span>
      <input
        className="h-11 min-w-0 rounded-md border border-input bg-background px-3 text-sm outline-2 outline-transparent focus-visible:outline-ring disabled:cursor-not-allowed disabled:opacity-50"
        value={value}
        onChange={(event) => onChange(event.target.value)}
        type={type}
        minLength={minLength}
        maxLength={maxLength}
        required={required}
        autoComplete="off"
      />
    </label>
  );
}
