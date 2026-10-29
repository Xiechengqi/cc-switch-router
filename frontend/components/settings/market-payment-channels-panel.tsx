"use client";

/* Hallmark · pre-emit critique: P4 H4 E4 S5 R4 V4 */
/* Hallmark · component: finance settings panel · genre: modern-minimal · theme: existing Router system
 * states: default · hover · focus · active · disabled · loading · error · success
 * contrast: inherited Router tokens
 */

import * as React from "react";
import { AlertTriangle, CheckCircle2, KeyRound, Loader2, Plus, RefreshCw, SlidersHorizontal } from "lucide-react";
import { Alert, Button, Modal } from "@heroui/react";
import { ConfirmAlertDialog } from "@/components/common/confirm-alert-dialog";
import { CompactSelect } from "@/components/common/compact-select";
import { CopyableCodeField } from "@/components/common/copyable-code-field";
import { WalletBadge } from "@/components/market/wallet-primitives";
import { MarketFinanceOverviewSection } from "@/components/settings/market-finance-overview";
import { WalletPolicySettings } from "@/components/settings/wallet-policy-settings";
import { MarketTrialSettings } from "@/components/settings/market-trial-settings";
import { ClientTerminationSettings } from "@/components/settings/client-termination-settings";
import { CommissionPolicySettings } from "@/components/settings/commission-policy-settings";
import { C2CFinanceSettings } from "@/components/settings/c2c-finance-settings";
import { MarketFinanceCaseDesk } from "@/components/settings/market-finance-case-desk";
import { MarketFinanceEnableDialog } from "@/components/settings/market-finance-enable-dialog";
import { MarketChannelSettingsDialog, channelSettingsSummary } from "@/components/settings/market-channel-settings-dialog";
import { MarketFinanceUserLookup } from "@/components/settings/market-finance-user-lookup";
import { MarketFinanceEarningHolds } from "@/components/settings/market-finance-earning-holds";
import { codeLabel, ownerDateText, ownerErrorText } from "@/components/settings/market-finance-shared";
import { MarketFinanceDisputesPanel } from "@/components/settings/market-finance-disputes-panel";
import { MarketFinanceTopupRecoveryPanel } from "@/components/settings/market-finance-topup-recovery-panel";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { ApiError } from "@/lib/api";
import {
  createMarketPaymentChannel,
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
  rewrapMarketPaymentChannelSecrets,
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

export function MarketPaymentChannelsPanel() {
  const { locale, t } = useLocaleText();
  const [readiness, setReadiness] = React.useState<MarketFinanceReadiness | null>(null);
  const [channels, setChannels] = React.useState<MarketPaymentChannel[]>([]);
  const [refreshKey, setRefreshKey] = React.useState(0);
  const [enableOpen, setEnableOpen] = React.useState(false);
  const [enableError, setEnableError] = React.useState("");
  const [settingsChannel, setSettingsChannel] = React.useState<MarketPaymentChannel | null>(null);
  const [rewrapOpen, setRewrapOpen] = React.useState(false);
  const [notice, setNotice] = React.useState("");
  const loadVersion = React.useRef(0);
  const [form, setForm] = React.useState<FormState>(EMPTY_FORM);
  const [dialog, setDialog] = React.useState<DialogState | null>(null);
  const [showArchived, setShowArchived] = React.useState(false);
  const [manageChannelId, setManageChannelId] = React.useState<string | null>(null);
  const [busy, setBusy] = React.useState("");
  const [error, setError] = React.useState("");
  const [loaded, setLoaded] = React.useState(false);
  const busyRef = React.useRef(false);

  // Readiness and channels drive this panel; the overview, case desk and
  // holds reload themselves whenever `refreshKey` changes.
  const reloadCore = React.useCallback(async () => {
    const version = ++loadVersion.current;
    const [nextReadiness, nextChannels] = await Promise.all([
      getMarketFinanceReadiness(),
      getMarketPaymentChannels(),
    ]);
    if (version !== loadVersion.current) return;
    setReadiness(nextReadiness);
    setChannels(nextChannels);
    setLoaded(true);
  }, []);

  const reload = React.useCallback(async () => {
    setRefreshKey((value) => value + 1);
    await reloadCore();
  }, [reloadCore]);

  React.useEffect(() => {
    let active = true;
    reloadCore().catch((cause) => {
      if (!active) return;
      setError(message(cause));
      setLoaded(true);
    });
    return () => { active = false; };
  }, [reloadCore]);

  const perform = async (name: string, action: () => Promise<unknown>) => {
    if (busyRef.current) return;
    busyRef.current = true;
    setBusy(name);
    setError("");
    setNotice("");
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
      } else if (cause instanceof ApiError && cause.code === "MARKET_PAYMENT_METHOD_CHANNEL_ACTIVE") {
        const method = cause.details?.method;
        setError(t("settings.marketPaymentChannels.methodActive", {
          method: method === "crypto" ? t("marketWallet.topupMethod.crypto") : String(method ?? ""),
          channel: String(cause.details?.displayName ?? cause.details?.channelId ?? ""),
        }));
      } else {
        setError(ownerErrorText(t, cause));
      }
    } finally {
      busyRef.current = false;
      setBusy("");
    }
  };

  const confirmEnable = async () => {
    if (!readiness || busyRef.current) return;
    busyRef.current = true;
    setBusy("enable");
    setEnableError("");
    try {
      await setMarketFinanceMode("enabled", readiness.version);
      setEnableOpen(false);
      await reload().catch(() => {});
    } catch (cause) {
      setEnableError(ownerErrorText(t, cause));
      await reloadCore().catch(() => {});
    } finally {
      busyRef.current = false;
      setBusy("");
    }
  };

  const setPause = (patch: Partial<Pick<MarketFinanceReadiness, "newOrdersPaused" | "newTopupsPaused">>) => {
    if (!readiness) return;
    const next = {
      newOrdersPaused: readiness.newOrdersPaused,
      newTopupsPaused: readiness.newTopupsPaused ?? false,
      ...patch,
    };
    void perform("pause", () => setMarketFinancePause(next.newOrdersPaused, readiness.version, next.newTopupsPaused));
  };

  const confirmRewrap = () => void perform("rewrap", async () => {
    const result = await rewrapMarketPaymentChannelSecrets();
    setRewrapOpen(false);
    setNotice(t("settings.marketFinance.rewrap.done", { count: result.rewrapped, version: result.keyVersion }));
  }).finally(() => setRewrapOpen(false));

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
      {error && !dialog && !settingsChannel ? <Alert status="danger" role="alert">{error}</Alert> : null}
      <div aria-live="polite">{notice ? <Alert status="success">{notice}</Alert> : null}</div>
      <MarketFinanceOverviewSection refreshKey={refreshKey} />
      <WalletPolicySettings />
      <MarketTrialSettings />
      <ClientTerminationSettings />
      <CommissionPolicySettings />
      <C2CFinanceSettings />

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
              {readiness.mode !== "disabled" ? (
                <Button size="sm" variant="outline" isDisabled={!!busy} onPress={() => void perform("mode", () => setMarketFinanceMode("disabled", readiness.version))}>
                  {t("settings.marketFinance.disable")}
                </Button>
              ) : null}
              {readiness.mode !== "shadow" ? (
                <Button size="sm" variant="outline" isDisabled={!!busy} onPress={() => void perform("mode", () => setMarketFinanceMode("shadow", readiness.version))}>
                  {t("settings.marketFinance.shadow")}
                </Button>
              ) : null}
              {readiness.mode === "shadow" ? (
                <Button size="sm" isDisabled={!!busy || !readiness.canEnable} onPress={() => { setEnableError(""); setEnableOpen(true); }}>
                  {t("settings.marketFinance.enable")}
                </Button>
              ) : null}
            </div>
          </div>
          <div className="grid gap-1 text-sm text-muted-foreground sm:grid-cols-2">
            <p>{t("settings.marketFinance.verifiedTopups", { count: readiness.verifiedLiveTopups })}</p>
            <p>{t("settings.marketFinance.channelBlockers", { count: readiness.paymentChannelBlockers.length })}</p>
            <p>{t("settings.marketFinance.criticalCases", { count: readiness.criticalCases })}</p>
            {readiness.staleTopupReviews > 0 ? <p className="font-medium text-amber-800">{t("settings.marketFinance.staleTopupReviews", { count: readiness.staleTopupReviews })}</p> : null}
          </div>
            <div className="grid gap-3 sm:grid-cols-2">
              {([
                ["newTopupsPaused", "settings.marketFinance.pause.newTopups", true],
                ["newOrdersPaused", "settings.marketFinance.newOrders", true],
              ] as const).filter(([, , shown]) => shown).map(([field, label]) => {
                const paused = !!readiness[field];
                return (
                  <div key={field} className="flex items-center justify-between gap-3">
                    <span className="flex flex-wrap items-center gap-2 text-sm">{t(label)}
                      <WalletBadge tone={paused ? "warning" : "success"}>
                        {t(paused ? "settings.marketFinance.statusPaused" : "settings.marketFinance.statusOpen")}
                      </WalletBadge>
                    </span>
                    <Button size="sm" variant="outline" className="min-h-11" isDisabled={!!busy} aria-pressed={paused} onPress={() => setPause({ [field]: !paused })}>
                      {t(paused ? "settings.marketFinance.resume" : "settings.marketFinance.pause")}
                    </Button>
                  </div>
                );
              })}
              <p className="text-xs text-muted-foreground sm:col-span-2">{t("settings.marketFinance.pause.topupNote")}</p>
              <p className="text-xs text-muted-foreground sm:col-span-2">{t("settings.marketFinance.pauseNote")}</p>
            </div>
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
          {readiness.paymentChannelBlockers.length ? (
            <ul className="grid gap-1 text-xs text-muted-foreground">
              {readiness.paymentChannelBlockers.map((blocker) => (
                <li key={blocker.code} className="flex justify-between gap-3">
                  <span>{codeLabel(t, "settings.marketFinance.blocker.", blocker.code)}</span>
                  <span className="tabular-nums">{blocker.count}</span>
                </li>
              ))}
            </ul>
          ) : null}
        </div>
      ) : null}

      <div className="grid min-w-0 gap-4">
        <div className="flex flex-wrap items-center justify-between gap-3">
          <h4 className="text-sm font-semibold">{t("settings.marketFinance.channels")}</h4>
          <div className="flex flex-wrap gap-2">
            <Button size="sm" variant="outline" isDisabled={!!busy} onPress={() => setRewrapOpen(true)}>
              <KeyRound className="size-4" aria-hidden="true" />{t("settings.marketFinance.rewrap.button")}
            </Button>
            <Button size="sm" isDisabled={!!busy} onPress={() => openDialog("create")}>
              <Plus className="size-4" />{t("settings.marketFinance.addChannel")}
            </Button>
          </div>
        </div>
        <p className="text-xs text-muted-foreground">{t("settings.marketFinance.channel.canaryNote")}</p>
        <p className="text-xs text-muted-foreground">{t("settings.marketPaymentChannels.singleMethod")}</p>
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
                  <div className="flex flex-wrap items-center gap-2">
                    {(() => {
                      const rollout = channel.rolloutState ?? (channel.enabled ? "live" : "off");
                      return <WalletBadge tone={rollout === "live" ? "success" : rollout === "canary" ? "accent" : "neutral"} pulse={rollout === "canary"}>
                        {t(`settings.marketFinance.channel.rollout.${rollout}`)}
                      </WalletBadge>;
                    })()}
                    <span className="rounded-full bg-muted px-2.5 py-1 text-xs font-medium">
                      {t(channel.enabled ? "settings.marketFinance.channelActive" : "settings.marketFinance.channelStopped")}
                    </span>
                  </div>
                </div>
                {channel.promotedAt ? <p className="text-xs text-muted-foreground">{t("settings.marketFinance.channel.promotedAt", { time: ownerDateText(channel.promotedAt, locale) })}</p> : null}
                {channel.settings ? <p className="text-xs text-muted-foreground">{channelSettingsSummary(channel, t)}</p> : null}
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
                  {channel.settings ? <Button size="sm" variant="outline" isDisabled={!!busy} onPress={() => { setError(""); setSettingsChannel(channel); }}>
                    <SlidersHorizontal className="size-4" aria-hidden="true" />{t("settings.marketFinance.channel.settings")}
                  </Button> : null}
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
                  <div className="grid gap-1 text-sm font-medium"><span>{t("settings.marketFinance.webhookMode")}</span>
                    <CompactSelect ariaLabel={t("settings.marketFinance.webhookMode")} className="w-full" triggerClassName="h-11" value={form.webhookMode} onChange={value => setForm((current) => ({ ...current, webhookMode: value as FormState["webhookMode"], webhookSecret: "" }))}
                      options={[{ value: "signed", label: t("settings.marketFinance.webhookMode.signed") }, { value: "poll_only", label: t("settings.marketFinance.webhookMode.poll_only") }]} />
                  </div>
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

      <MarketChannelSettingsDialog channel={settingsChannel} onClose={() => setSettingsChannel(null)} onSaved={() => reloadCore().then(() => setNotice(t("settings.marketFinance.channel.settingsSaved")))} />
      <ConfirmAlertDialog
        open={rewrapOpen}
        title={t("settings.marketFinance.rewrap.title")}
        description={t("settings.marketFinance.rewrap.note")}
        confirmLabel={t("settings.marketFinance.rewrap.button")}
        cancelLabel={t("common.cancel")}
        busy={busy === "rewrap"}
        onConfirm={confirmRewrap}
        onOpenChange={(open) => { if (busy !== "rewrap") setRewrapOpen(open); }}
      />
      {readiness ? <MarketFinanceEnableDialog
        open={enableOpen}
        readiness={readiness}
        busy={busy === "enable"}
        error={enableError}
        onConfirm={() => void confirmEnable()}
        onClose={() => setEnableOpen(false)}
      /> : null}

      <MarketFinanceCaseDesk refreshKey={refreshKey} onChanged={reloadCore} />
      <MarketFinanceUserLookup />
      <MarketFinanceEarningHolds refreshKey={refreshKey} onChanged={reloadCore} />
      <MarketFinanceTopupRecoveryPanel onRecovered={reload} />
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
