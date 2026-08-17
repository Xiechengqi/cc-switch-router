"use client";

import * as React from "react";
import { Button, Switch } from "@heroui/react";
import { Bell, ExternalLink, Loader2, Mail, Send, Unlink } from "lucide-react";
import { useAuth } from "@/components/auth/auth-provider";
import { ConfirmAlertDialog } from "@/components/common/confirm-alert-dialog";
import { useLocaleText } from "@/components/i18n/locale-provider";
import {
  createTelegramBindLink,
  getMyNotificationSettings,
  unbindMyTelegramChat,
  updateMyNotificationSettings,
} from "@/lib/api";
import type { NotificationChannelSettings, NotificationSettings, TelegramBindLink } from "@/lib/types";
import { cn, formatDateTime } from "@/lib/utils";

/** How long to keep watching for the `/start` handshake after opening the tab. */
const BIND_POLL_INTERVAL_MS = 3_000;
const BIND_POLL_TIMEOUT_MS = 5 * 60_000;
const BOT_STATUS_POLL_INTERVAL_MS = 3_000;
const BOT_ERROR_STATUS_POLL_INTERVAL_MS = 10_000;

type ChannelOption = {
  value: string;
  icon: typeof Mail;
  labelKey: "account.notifications.channel.email" | "account.notifications.channel.telegram";
  hintKey:
    | "account.notifications.channel.emailHint"
    | "account.notifications.channel.telegramHint";
};

const CHANNEL_OPTIONS: ChannelOption[] = [
  {
    value: "email",
    icon: Mail,
    labelKey: "account.notifications.channel.email",
    hintKey: "account.notifications.channel.emailHint",
  },
  {
    value: "telegram",
    icon: Send,
    labelKey: "account.notifications.channel.telegram",
    hintKey: "account.notifications.channel.telegramHint",
  },
];

export function AccountNotificationsPanel() {
  const { locale, t } = useLocaleText();
  const { session, loading: authLoading } = useAuth();
  const authed = !!session?.authenticated;

  const [settings, setSettings] = React.useState<NotificationSettings | null>(null);
  const [bindLink, setBindLink] = React.useState<TelegramBindLink | null>(null);
  const [loading, setLoading] = React.useState(true);
  const [busy, setBusy] = React.useState(false);
  const [waitingForBind, setWaitingForBind] = React.useState(false);
  const [error, setError] = React.useState("");
  const [unbindOpen, setUnbindOpen] = React.useState(false);
  const [bindBaselineVerifiedAt, setBindBaselineVerifiedAt] = React.useState<string | undefined>();

  const load = React.useCallback(async () => {
    setError("");
    try {
      setSettings(await getMyNotificationSettings());
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  }, []);

  React.useEffect(() => {
    if (!authed) return;
    load().catch(console.error);
  }, [authed, load]);

  React.useEffect(() => {
    const status = settings?.telegramBotStatus;
    if (!authed || !settings?.telegramBotConfigured || status === "ready") return;
    let active = true;
    let refreshing = false;
    const refresh = () => {
      if (refreshing) return;
      refreshing = true;
      void getMyNotificationSettings()
        .then((next) => {
          if (active) setSettings(next);
        })
        .catch(() => undefined)
        .finally(() => {
          refreshing = false;
        });
    };
    const timer = window.setInterval(
      refresh,
      status === "error" ? BOT_ERROR_STATUS_POLL_INTERVAL_MS : BOT_STATUS_POLL_INTERVAL_MS,
    );
    return () => {
      active = false;
      window.clearInterval(timer);
    };
  }, [authed, settings?.telegramBotConfigured, settings?.telegramBotStatus]);

  // The binding is completed in Telegram, not here: the only way this page
  // learns about it is by asking again until the chat shows up.
  React.useEffect(() => {
    if (!waitingForBind) return;
    let active = true;
    const startedAt = Date.now();
    const timer = window.setInterval(() => {
      if (!active) return;
      if (Date.now() - startedAt > BIND_POLL_TIMEOUT_MS) {
        setWaitingForBind(false);
        setBindBaselineVerifiedAt(undefined);
        return;
      }
      void getMyNotificationSettings()
        .then((next) => {
          if (!active) return;
          setSettings(next);
          const telegram = channelSettings(next, "telegram");
          if (
            telegram?.state === "ready"
            && (!bindBaselineVerifiedAt || telegram.verifiedAt !== bindBaselineVerifiedAt)
          ) {
            setWaitingForBind(false);
            setBindLink(null);
            setBindBaselineVerifiedAt(undefined);
          }
        })
        .catch(() => undefined);
    }, BIND_POLL_INTERVAL_MS);
    return () => {
      active = false;
      window.clearInterval(timer);
    };
  }, [bindBaselineVerifiedAt, waitingForBind]);

  const toggleChannel = async (channel: string, enabled: boolean) => {
    if (!settings || busy) return;
    const next = new Set(settings.enabledChannels);
    if (enabled) next.add(channel);
    else next.delete(channel);
    if (next.size === 0) return;
    setBusy(true);
    setError("");
    try {
      setSettings(await updateMyNotificationSettings([...next].sort()));
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const startBinding = async () => {
    setBusy(true);
    setError("");
    // Opened before the await so the browser still attributes the tab to the
    // click; a blocked popup falls back to the link rendered below.
    const tab = window.open("about:blank", "_blank");
    if (tab) tab.opener = null;
    try {
      setBindBaselineVerifiedAt(channelSettings(settings, "telegram")?.verifiedAt);
      const link = await createTelegramBindLink();
      setBindLink(link);
      setWaitingForBind(true);
      if (tab) tab.location.href = link.url;
    } catch (err) {
      tab?.close();
      setBindBaselineVerifiedAt(undefined);
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const unbind = async () => {
    setBusy(true);
    setError("");
    try {
      setSettings(await unbindMyTelegramChat());
      setBindLink(null);
      setWaitingForBind(false);
      setBindBaselineVerifiedAt(undefined);
      setUnbindOpen(false);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  if (authLoading) {
    return (
      <div className="flex items-center gap-2 py-6 text-sm text-muted-foreground">
        <Loader2 className="h-4 w-4 animate-spin" />
        {t("account.loading")}
      </div>
    );
  }

  if (!authed) {
    return <p className="py-6 text-sm text-muted-foreground">{t("account.apiKeys.signInRequired")}</p>;
  }

  if (loading && !settings) {
    return (
      <div className="flex items-center gap-2 py-6 text-sm text-muted-foreground">
        <Loader2 className="h-4 w-4 animate-spin" />
        {t("account.loading")}
      </div>
    );
  }

  const telegram = channelSettings(settings, "telegram");
  const botStatus = settings?.telegramBotStatus ?? "disabled";
  const botConfigured = settings?.telegramBotConfigured === true;
  const botReady = botConfigured
    && botStatus === "ready"
    && telegram?.available === true
    && !!settings?.telegramBotUsername;
  const botReconciling = botConfigured && (botStatus === "reconciling" || botStatus === "disabled");
  const botError = botStatus === "error";
  const bound = telegram?.state === "ready";
  const botHint = botReady
    ? t("account.notifications.telegramHint")
    : botReconciling
      ? t("account.notifications.telegramReconciling")
      : botError
        ? t("account.notifications.telegramError")
        : botStatus === "disabled"
          ? t("account.notifications.telegramDisabled")
          : t("account.notifications.telegramUnavailable");
  const botStatusLabel = botReady
    ? t("account.notifications.botReady")
    : botReconciling
      ? t("account.notifications.botReconciling")
      : botError
        ? t("account.notifications.botError")
        : t("account.notifications.botDisabled");

  return (
    <div className="grid min-w-0 gap-6">
      <div>
        <h2 className="flex items-center gap-2 text-base font-semibold text-foreground">
          <Bell className="h-4 w-4 text-muted-foreground" aria-hidden />
          {t("account.notifications.title")}
        </h2>
        <p className="mt-0.5 text-sm text-muted-foreground">{t("account.notifications.hint")}</p>
      </div>

      {error ? (
        <div className="rounded-lg border border-red-200 bg-red-50 px-3 py-2 text-sm text-red-700" role="alert">
          {error}
        </div>
      ) : null}

      <section className="grid gap-3">
        <div>
          <h3 className="text-sm font-semibold text-foreground">{t("account.notifications.channelTitle")}</h3>
          <p className="mt-0.5 text-sm text-muted-foreground">
            {t("account.notifications.channelHint")}
          </p>
        </div>
        <div className="divide-y rounded-lg border border-border bg-card">
          {CHANNEL_OPTIONS.map((option) => {
            const Icon = option.icon;
            const channel = channelSettings(settings, option.value);
            const active = !!channel?.enabled;
            const onlyEnabled = active && settings?.enabledChannels.length === 1;
            const cannotEnable = !active && (!channel?.available || channel.state !== "ready");
            const disabled = busy || onlyEnabled || cannotEnable;
            return (
              <div
                key={option.value}
                className="flex min-h-16 items-center gap-3 px-3 py-3 sm:px-4"
              >
                <Icon className={cn("h-4 w-4 shrink-0", active ? "text-sky-600" : "text-muted-foreground")} aria-hidden />
                <div className="min-w-0 flex-1">
                  <div className="text-sm font-medium text-foreground">{t(option.labelKey)}</div>
                  <div className="text-xs leading-relaxed text-muted-foreground">{t(option.hintKey)}</div>
                </div>
                <Switch
                  aria-label={t(option.labelKey)}
                  isSelected={active}
                  isDisabled={disabled}
                  onChange={(selected: boolean) => void toggleChannel(option.value, selected)}
                />
              </div>
            );
          })}
        </div>
        <p className="text-xs text-muted-foreground">
          {t("account.notifications.emailTarget")}
          <span className="ml-1 font-mono text-foreground">{settings?.email}</span>
        </p>
      </section>

      <section className="grid gap-3 rounded-xl border border-border bg-card p-4 sm:p-5">
        <div className="flex flex-wrap items-start justify-between gap-3">
          <div className="min-w-0">
            <h3 className="flex items-center gap-2 text-sm font-semibold text-foreground">
              <Send className="h-4 w-4 text-muted-foreground" aria-hidden />
              {t("account.notifications.telegramTitle")}
            </h3>
            <p className="mt-0.5 text-sm text-muted-foreground">{botHint}</p>
          </div>
          <div className="flex flex-wrap items-center justify-end gap-2">
            <span
              className={cn(
                "inline-flex shrink-0 items-center gap-1.5 rounded-full px-2.5 py-1 text-xs font-medium",
                botReady
                  ? "bg-emerald-50 text-emerald-700"
                  : botError
                    ? "bg-red-50 text-red-700"
                    : botReconciling
                      ? "bg-amber-50 text-amber-700"
                      : "bg-slate-100 text-slate-600",
              )}
            >
              {botReconciling ? (
                <Loader2 className="h-3 w-3 animate-spin" aria-hidden />
              ) : (
                <span
                  className={cn(
                    "h-1.5 w-1.5 rounded-full",
                    botReady
                      ? "bg-emerald-500"
                      : botError
                        ? "bg-red-500"
                        : "bg-slate-400",
                  )}
                  aria-hidden
                />
              )}
              {botStatusLabel}
            </span>
            <span
              className={cn(
                "inline-flex shrink-0 items-center gap-1.5 rounded-full px-2.5 py-1 text-xs font-medium",
                bound ? "bg-emerald-50 text-emerald-700" : "bg-slate-100 text-slate-600",
              )}
            >
              <span className={cn("h-1.5 w-1.5 rounded-full", bound ? "bg-emerald-500" : "bg-slate-400")} aria-hidden />
              {bound ? t("account.notifications.bound") : t("account.notifications.notBound")}
            </span>
          </div>
        </div>

        {bound ? (
          <div className="grid gap-2 text-sm">
            {telegram?.targetLabel ? (
              <div className="flex justify-between gap-3">
                <span className="text-muted-foreground">{t("account.notifications.telegramAccount")}</span>
                <span className="font-mono">@{telegram.targetLabel}</span>
              </div>
            ) : null}
            {telegram?.verifiedAt ? (
              <div className="flex justify-between gap-3">
                <span className="text-muted-foreground">{t("account.notifications.boundAt")}</span>
                <span className="tabular-nums">{formatDateTime(telegram.verifiedAt, locale)}</span>
              </div>
            ) : null}
          </div>
        ) : null}

        <div className="flex flex-wrap items-center gap-2">
          <Button
            size="sm"
            variant={bound ? "outline" : "primary"}
            isDisabled={!botReady || busy}
            onClick={() => void startBinding()}
          >
            {busy || botReconciling ? <Loader2 className="h-4 w-4 animate-spin" /> : <ExternalLink className="h-4 w-4" />}
            {bound ? t("account.notifications.rebind") : t("account.notifications.bind")}
          </Button>
          {bound ? (
            <Button
              size="sm"
              variant="outline"
              className="text-rose-700"
              isDisabled={busy}
              onClick={() => setUnbindOpen(true)}
            >
              <Unlink className="h-4 w-4" />
              {t("account.notifications.unbind")}
            </Button>
          ) : null}
          {waitingForBind ? (
            <span className="inline-flex items-center gap-1.5 text-xs text-muted-foreground">
              <Loader2 className="h-3.5 w-3.5 animate-spin" aria-hidden />
              {t("account.notifications.waiting")}
            </span>
          ) : null}
        </div>

        {bindLink && !bound ? (
          <div className="grid gap-2 rounded-lg border border-border bg-muted/40 px-3 py-2 text-xs">
            <p className="text-muted-foreground">{t("account.notifications.manualHint")}</p>
            <a
              href={bindLink.url}
              target="_blank"
              rel="noreferrer noopener"
              className="break-all font-mono text-sky-700 underline underline-offset-2"
            >
              {bindLink.url}
            </a>
            <p className="font-mono text-foreground">/start {bindLink.token}</p>
            <p className="text-muted-foreground">
              {t("account.notifications.expiresAt")}
              <span className="ml-1 tabular-nums">{formatDateTime(bindLink.expiresAt, locale)}</span>
            </p>
          </div>
        ) : null}
      </section>

      <ConfirmAlertDialog
        open={unbindOpen}
        title={t("account.notifications.unbindConfirmTitle")}
        description={t("account.notifications.unbindConfirmDescription")}
        confirmLabel={t("account.notifications.unbind")}
        cancelLabel={t("common.cancel")}
        tone="danger"
        busy={busy}
        onConfirm={() => void unbind()}
        onOpenChange={(next) => !busy && setUnbindOpen(next)}
      />
    </div>
  );
}

function channelSettings(
  settings: NotificationSettings | null | undefined,
  channel: string,
): NotificationChannelSettings | undefined {
  return settings?.channels.find((entry) => entry.channel === channel);
}
