"use client";

/* Hallmark · component: finance settings panel · genre: modern-minimal · theme: existing Router system
 * states: default · hover · focus · active · disabled · loading · error · success
 * contrast: inherited Router tokens
 */

import * as React from "react";
import { AlertTriangle, CheckCircle2, Loader2, Plus, RefreshCw } from "lucide-react";
import { Alert, Button } from "@heroui/react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import {
  createMarketPaymentChannel,
  getMarketFinanceCases,
  getMarketFinanceReadiness,
  getMarketPaymentChannels,
  setMarketFinanceMode,
  setMarketPaymentChannelEnabled,
  type MarketFinanceCase,
  type MarketFinanceReadiness,
  type MarketPaymentChannel,
} from "@/lib/market-wallet-api";

type FormState = {
  displayName: string;
  apiKeyId: string;
  apiSecret: string;
  webhookSecret: string;
};

const EMPTY_FORM: FormState = {
  displayName: "DujiaoPay",
  apiKeyId: "",
  apiSecret: "",
  webhookSecret: "",
};

function message(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

export function MarketPaymentChannelsPanel() {
  const { t } = useLocaleText();
  const [readiness, setReadiness] = React.useState<MarketFinanceReadiness | null>(null);
  const [channels, setChannels] = React.useState<MarketPaymentChannel[]>([]);
  const [cases, setCases] = React.useState<MarketFinanceCase[]>([]);
  const [form, setForm] = React.useState<FormState>(EMPTY_FORM);
  const [busy, setBusy] = React.useState("");
  const [error, setError] = React.useState("");
  const [notice, setNotice] = React.useState("");
  const [loaded, setLoaded] = React.useState(false);
  const busyRef = React.useRef(false);

  const reload = React.useCallback(async () => {
    const [nextReadiness, nextChannels, nextCases] = await Promise.all([
      getMarketFinanceReadiness(),
      getMarketPaymentChannels(),
      getMarketFinanceCases(),
    ]);
    setReadiness(nextReadiness);
    setChannels(nextChannels);
    setCases(nextCases);
    setLoaded(true);
  }, []);

  React.useEffect(() => {
    let active = true;
    Promise.all([
      getMarketFinanceReadiness(),
      getMarketPaymentChannels(),
      getMarketFinanceCases(),
    ]).then(([nextReadiness, nextChannels, nextCases]) => {
      if (!active) return;
      setReadiness(nextReadiness);
      setChannels(nextChannels);
      setCases(nextCases);
      setLoaded(true);
    }).catch((cause) => {
      if (!active) return;
      setError(message(cause));
      setLoaded(true);
    });
    return () => { active = false; };
  }, []);

  const perform = async (name: string, action: () => Promise<unknown>, success = "") => {
    if (busyRef.current) return;
    busyRef.current = true;
    setBusy(name);
    setError("");
    setNotice("");
    try {
      await action();
      await reload();
      if (success) setNotice(success);
    } catch (cause) {
      setError(message(cause));
    } finally {
      busyRef.current = false;
      setBusy("");
    }
  };

  const createChannel = async (event: React.FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    await perform("create", async () => {
      await createMarketPaymentChannel({
        displayName: form.displayName.trim(),
        sortOrder: channels.length,
        apiKeyId: form.apiKeyId.trim(),
        apiSecret: form.apiSecret,
        webhookSecret: form.webhookSecret,
        allowedMethods: [],
      });
      setForm(EMPTY_FORM);
    }, t("settings.marketFinance.channelCreated"));
  };

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
      {error ? <Alert status="danger" role="alert">{error}</Alert> : null}
      {notice ? <Alert status="success" role="status">{notice}</Alert> : null}

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
          </div>
          {!readiness.canEnable ? (
            <p className="flex items-start gap-2 text-sm text-muted-foreground">
              <AlertTriangle className="mt-0.5 size-4 shrink-0" aria-hidden="true" />
              {t("settings.marketFinance.cutoverBlocked")}
            </p>
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
        </div>
      ) : null}

      <div className="grid min-w-0 gap-4">
        <h4 className="text-sm font-semibold">{t("settings.marketFinance.channels")}</h4>
        {channels.length ? (
          <ul className="grid gap-2">
            {channels.map((channel) => (
              <li key={channel.id} className="flex min-w-0 flex-wrap items-center justify-between gap-3 rounded-lg border border-border bg-card px-4 py-3">
                <div className="min-w-0">
                  <p className="truncate text-sm font-medium">{channel.displayName}</p>
                  <p className="flex items-center gap-1 text-xs text-muted-foreground">
                    {channel.verified ? <CheckCircle2 className="size-3.5" aria-hidden="true" /> : <AlertTriangle className="size-3.5" aria-hidden="true" />}
                    {channel.verified ? t("settings.marketFinance.verified") : t("settings.marketFinance.unverified")}
                    <span aria-hidden="true">·</span> v{channel.currentRevision}
                  </p>
                </div>
                <Button
                  size="sm"
                  variant="outline"
                  isDisabled={!!busy || !channel.verified}
                  onPress={() => void perform(channel.id, () => setMarketPaymentChannelEnabled(channel.id, !channel.enabled))}
                >
                  {channel.enabled ? t("settings.marketFinance.stopChannel") : t("settings.marketFinance.startChannel")}
                </Button>
              </li>
            ))}
          </ul>
        ) : loaded ? <p className="text-sm text-muted-foreground">{t("settings.marketFinance.noChannels")}</p> : null}

        <form onSubmit={(event) => void createChannel(event)} className="grid min-w-0 gap-3 rounded-lg border border-border bg-card p-4 sm:grid-cols-2 sm:p-5">
          <h5 className="sm:col-span-2 text-sm font-semibold">{t("settings.marketFinance.addChannel")}</h5>
          <Field label={t("settings.marketFinance.displayName")} value={form.displayName} onChange={(value) => setForm((current) => ({ ...current, displayName: value }))} required />
          <Field label={t("settings.marketFinance.keyId")} value={form.apiKeyId} onChange={(value) => setForm((current) => ({ ...current, apiKeyId: value }))} required />
          <Field label={t("settings.marketFinance.apiSecret")} value={form.apiSecret} onChange={(value) => setForm((current) => ({ ...current, apiSecret: value }))} type="password" minLength={16} required />
          <Field label={t("settings.marketFinance.webhookSecret")} value={form.webhookSecret} onChange={(value) => setForm((current) => ({ ...current, webhookSecret: value }))} type="password" minLength={16} required />
          <p className="sm:col-span-2 text-xs text-muted-foreground">{t("settings.marketFinance.keyNote")}</p>
          <div className="sm:col-span-2 flex justify-start">
            <Button type="submit" isDisabled={!!busy || !form.displayName.trim() || !form.apiKeyId.trim() || form.apiSecret.length < 16 || form.webhookSecret.length < 16}>
              {busy === "create" ? <Loader2 className="size-4 animate-spin" /> : <Plus className="size-4" />}
              {busy === "create" ? t("settings.marketFinance.saving") : t("settings.marketFinance.saveChannel")}
            </Button>
          </div>
        </form>
      </div>

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
          <p className="text-xs text-muted-foreground">{t("settings.marketFinance.caseNote")}</p>
        </div>
      ) : null}
    </section>
  );
}

function Field({ label, value, onChange, type = "text", minLength, required }: {
  label: string;
  value: string;
  onChange: (value: string) => void;
  type?: "text" | "password";
  minLength?: number;
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
        required={required}
        autoComplete="off"
      />
    </label>
  );
}
