"use client";

import { Alert, Button, Chip } from "@heroui/react";
import { Bell, Bot, ExternalLink, Loader2, RefreshCw, Send } from "lucide-react";
import Link from "next/link";
import * as React from "react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { alertChannelLabel } from "@/lib/alerting";
import {
  getAlertingChannels,
  getUserNotificationChannels,
  testAlertingChannel,
  testUserNotificationChannel,
} from "@/lib/api";
import { DASHBOARD_ACCOUNT_NOTIFICATIONS_PATH } from "@/lib/dashboard-nav";
import type { AlertChannelState, UserNotificationChannelState } from "@/lib/types";
import { formatDateTime } from "@/lib/utils";

export function AlertChannelsPanel({
  channel,
  refreshToken,
}: {
  channel?: string;
  refreshToken: number;
}) {
  const { t } = useLocaleText();
  const [operatorChannels, setOperatorChannels] = React.useState<AlertChannelState[]>([]);
  const [userChannels, setUserChannels] = React.useState<UserNotificationChannelState[]>([]);
  const [busy, setBusy] = React.useState("");
  const [error, setError] = React.useState("");
  const [success, setSuccess] = React.useState("");

  const load = React.useCallback(async () => {
    setBusy((current) => current || "load");
    try {
      const [operator, user] = await Promise.allSettled([
        getAlertingChannels(),
        getUserNotificationChannels(),
      ]);
      const failures: string[] = [];
      if (operator.status === "fulfilled") {
        setOperatorChannels(operator.value);
      } else {
        setOperatorChannels([]);
        failures.push(errorMessage(operator.reason));
      }
      if (user.status === "fulfilled") {
        setUserChannels(user.value);
      } else {
        setUserChannels([]);
        failures.push(errorMessage(user.reason));
      }
      setError(failures.join(" · "));
    } finally {
      setBusy("");
    }
  }, []);

  React.useEffect(() => {
    void load();
  }, [load, refreshToken]);

  const visibleOperator = channel
    ? operatorChannels.filter((item) => item.channel === channel)
    : operatorChannels;
  const visibleUser = channel
    ? userChannels.filter((item) => item.channel === channel)
    : userChannels;

  return (
    <section className="grid gap-4">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <h3 className="text-sm font-semibold">{t("settings.alertChannels.title")}</h3>
          <p className="mt-1 text-sm text-muted-foreground">{t("settings.alertChannels.description")}</p>
        </div>
        <Button variant="ghost" size="sm" onClick={() => void load()} isDisabled={!!busy}>
          {busy === "load" ? <Loader2 className="h-4 w-4 animate-spin" /> : <RefreshCw className="h-4 w-4" />}
          {t("common.reload")}
        </Button>
      </div>

      {error ? <Alert status="danger" className="!text-slate-900">{error}</Alert> : null}
      {success ? <Alert status="success" className="!text-slate-900">{success}</Alert> : null}

      <div className="grid gap-3">
        {visibleOperator.map((item) => {
          const testing = busy === `test:operator:${item.channel}`;
          const channelLabel = alertChannelLabel(item.channel);
          return (
            <div key={`operator:${item.channel}`} className="grid gap-4 rounded-md border bg-background p-4 md:grid-cols-[minmax(0,1fr)_auto] md:items-center">
              <div className="min-w-0">
                <div className="flex flex-wrap items-center gap-2">
                  <Bell className="h-4 w-4 text-muted-foreground" />
                  <span className="font-medium">
                    {t("settings.alertChannels.operator.title", { channel: channelLabel })}
                  </span>
                  <ChannelStatusChip state={item} />
                </div>
                <p className="mt-1 text-sm text-muted-foreground">
                  {t(item.channel === "telegram"
                    ? "settings.alertChannels.operator.telegramDescription"
                    : "settings.alertChannels.operator.description")}
                </p>
                <p className="mt-1 text-xs text-muted-foreground">
                  {item.lastSuccessAt
                    ? t("settings.alertChannels.lastSuccess", { time: formatDateTime(item.lastSuccessAt * 1000) })
                    : t("settings.alertChannels.neverSucceeded")}
                </p>
                {item.lastError ? <p className="mt-1 break-words text-xs text-red-600">{item.lastError}</p> : null}
              </div>
              <Button
                variant="outline"
                size="sm"
                onClick={() => void runOperatorTest(item.channel)}
                isDisabled={!!busy || !item.configured}
              >
                {testing ? <Loader2 className="h-4 w-4 animate-spin" /> : <Send className="h-4 w-4" />}
                {t("settings.alertChannels.sendTest")}
              </Button>
            </div>
          );
        })}
        {visibleUser.map((item) => {
          const testing = busy === `test:user:${item.channel}`;
          const channelLabel = alertChannelLabel(item.channel);
          const target = channelValueLabel(item.channel, item.testTargetLabel);
          const provider = channelValueLabel(item.channel, item.providerLabel);
          return (
            <div key={`user:${item.channel}`} className="grid gap-4 rounded-md border bg-background p-4 md:grid-cols-[minmax(0,1fr)_auto] md:items-center">
              <div className="min-w-0">
                <div className="flex flex-wrap items-center gap-2">
                  <Bot className="h-4 w-4 text-muted-foreground" />
                  <span className="font-medium">
                    {t("settings.alertChannels.user.title", { channel: channelLabel })}
                  </span>
                  <ChannelStatusChip state={item} />
                </div>
                <p className="mt-1 text-sm text-muted-foreground">
                  {t(item.channel === "telegram"
                    ? "settings.alertChannels.user.telegramDescription"
                    : "settings.alertChannels.user.description")}
                </p>
                <div className="mt-2 flex flex-wrap gap-x-4 gap-y-1 text-xs text-muted-foreground">
                  {provider ? <span>{t("settings.alertChannels.user.provider", { provider })}</span> : null}
                  {item.runtimeVerifiedAt ? (
                    <span>{t("settings.alertChannels.user.runtimeVerified", { time: formatDateTime(item.runtimeVerifiedAt) })}</span>
                  ) : null}
                  {item.testTargetAvailable ? (
                    <span>{t("settings.alertChannels.user.target", { target: target || t("settings.alertChannels.user.privateTarget") })}</span>
                  ) : null}
                </div>
                <p className="mt-1 text-xs text-muted-foreground">
                  {item.lastSuccessAt
                    ? t("settings.alertChannels.lastSuccess", { time: formatDateTime(item.lastSuccessAt) })
                    : t("settings.alertChannels.neverSucceeded")}
                </p>
                {item.lastError ? <p className="mt-1 break-words text-xs text-red-600">{item.lastError}</p> : null}
                {item.runtimeReady && !item.testTargetAvailable ? (
                  <p className="mt-2 text-xs text-muted-foreground">
                    {t("settings.alertChannels.user.bindingRequired")}{" "}
                    <Link href={DASHBOARD_ACCOUNT_NOTIFICATIONS_PATH} className="inline-flex items-center gap-1 font-medium text-foreground underline underline-offset-2">
                      {t("settings.alertChannels.user.bindAction")}
                      <ExternalLink className="h-3 w-3" aria-hidden />
                    </Link>
                  </p>
                ) : null}
                {!item.runtimeReady ? (
                  <p className="mt-2 text-xs text-muted-foreground">
                    {t("settings.alertChannels.user.runtimeUnavailable")}
                  </p>
                ) : null}
              </div>
              <Button
                variant="outline"
                size="sm"
                onClick={() => void runUserTest(item.channel)}
                isDisabled={!!busy || !item.testTargetAvailable}
              >
                {testing ? <Loader2 className="h-4 w-4 animate-spin" /> : <Send className="h-4 w-4" />}
                {t("settings.alertChannels.sendTest")}
              </Button>
            </div>
          );
        })}
        {busy === "load" && visibleOperator.length === 0 && visibleUser.length === 0 ? (
          <p className="text-sm text-muted-foreground">{t("common.loading")}</p>
        ) : null}
      </div>
    </section>
  );

  async function runOperatorTest(target: string) {
    setBusy(`test:operator:${target}`);
    setError("");
    setSuccess("");
    try {
      await testAlertingChannel(target);
      setSuccess(t("settings.alertChannels.operator.testSent"));
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy("");
      await load();
    }
  }

  async function runUserTest(target: string) {
    setBusy(`test:user:${target}`);
    setError("");
    setSuccess("");
    try {
      const result = await testUserNotificationChannel(target);
      setSuccess(t("settings.alertChannels.user.testSent", {
        target: channelValueLabel(target, result.targetLabel) || t("settings.alertChannels.user.privateTarget"),
      }));
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy("");
      await load();
    }
  }
}

function ChannelStatusChip({ state }: { state: Pick<AlertChannelState, "status"> }) {
  const { t } = useLocaleText();
  const color = state.status === "healthy"
    ? "success"
    : state.status === "degraded" || state.status === "misconfigured"
      ? "danger"
      : state.status === "ready"
        ? "accent"
        : "default";
  return (
    <Chip color={color} size="sm" variant="soft">
      {t(`settings.alertChannels.status.${state.status}` as Parameters<typeof t>[0])}
    </Chip>
  );
}

function channelValueLabel(channel: string, value?: string | null) {
  const normalized = value?.trim();
  if (!normalized) return "";
  if (channel !== "telegram") return normalized;
  return `@${normalized.replace(/^@+/, "")}`;
}

function errorMessage(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}
