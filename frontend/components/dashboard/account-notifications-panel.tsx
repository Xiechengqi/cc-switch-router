"use client";

import * as React from "react";
import { Button } from "@heroui/react";
import { AlertTriangle, Bell, Check, ExternalLink, Loader2, Mail, Send, Unlink } from "lucide-react";
import { useAuth } from "@/components/auth/auth-provider";
import { ConfirmAlertDialog } from "@/components/common/confirm-alert-dialog";
import { useLocaleText } from "@/components/i18n/locale-provider";
import {
  ApiError,
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
      setError(formatTelegramError(err, t));
    } finally {
      setLoading(false);
    }
  }, [t]);

  React.useEffect(() => {
    if (!authed) return;
    load().catch(console.error);
  }, [authed, load]);

  React.useEffect(() => {
    const status = settings?.telegramBotStatus;
    const transportStatus = settings?.telegramBotTransportStatus;
    const transportDegraded = transportStatus === "degraded";
    if (!authed || !settings?.telegramBotConfigured || (status === "ready" && !transportDegraded)) {
      return;
    }
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
      status === "error" || transportDegraded
        ? BOT_ERROR_STATUS_POLL_INTERVAL_MS
        : BOT_STATUS_POLL_INTERVAL_MS,
    );
    return () => {
      active = false;
      window.clearInterval(timer);
    };
  }, [
    authed,
    settings?.telegramBotConfigured,
    settings?.telegramBotStatus,
    settings?.telegramBotTransportStatus,
  ]);

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

  // One channel at a time: picking a destination replaces the previous one
  // rather than adding to it, which is exactly what the API models.
  const selectChannel = async (channel: string) => {
    if (!settings || busy || settings.deliveryChannel === channel) return;
    setBusy(true);
    setError("");
    try {
      setSettings(await updateMyNotificationSettings(channel));
    } catch (err) {
      setError(formatTelegramError(err, t));
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
      setError(formatTelegramError(err, t));
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
      setError(formatTelegramError(err, t));
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
  const botTransportDegraded = settings?.telegramBotTransportStatus === "degraded";
  const botReady = botConfigured
    && botStatus === "ready"
    && !botTransportDegraded
    && telegram?.available === true
    && !!settings?.telegramBotUsername;
  const botReconciling = botConfigured && (botStatus === "reconciling" || botStatus === "disabled");
  const botError = botStatus === "error" || botTransportDegraded;
  const bound = telegram?.state === "ready";
  const selectedChannel = settings?.deliveryChannel ?? "email";
  // The backend silently delivers to email when the selection cannot carry the
  // alert. Say so, instead of showing a Telegram selection that is not running.
  const fallbackActive = selectedChannel === "telegram" && telegram?.available !== true;
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
        <div
          role="radiogroup"
          aria-label={t("account.notifications.channelTitle")}
          aria-busy={busy}
          className="grid gap-2"
        >
          {CHANNEL_OPTIONS.map((option) => {
            const Icon = option.icon;
            const channel = channelSettings(settings, option.value);
            const selected = selectedChannel === option.value;
            // Email is always a valid destination; Telegram needs a live bot
            // and a bound chat before it can carry anything.
            const usable = option.value === "email"
              || (channel?.state === "ready" && channel?.available === true);
            const reason = usable
              ? ""
              : channel?.state === "ready"
                ? t("account.notifications.channel.botUnavailable")
                : t("account.notifications.channel.needsBinding");
            // Not disabled while busy: `selectChannel` already ignores the
            // change, and disabling the focused radio mid-request would drop
            // keyboard focus out of the group.
            const disabled = !usable && !selected;
            return (
              <label
                key={option.value}
                className={cn(
                  "group relative flex min-h-16 cursor-pointer items-center gap-3 rounded-xl border px-3 py-3 transition-all duration-200 sm:px-4",
                  selected
                    ? "border-accent bg-accent/5 shadow-[0_4px_14px_rgba(0,82,255,0.15)]"
                    : "border-border bg-card hover:border-accent/30 hover:shadow-md",
                  disabled && !selected && "cursor-not-allowed opacity-60 hover:border-border hover:shadow-none",
                  "focus-within:ring-2 focus-within:ring-accent focus-within:ring-offset-2",
                )}
              >
                <input
                  type="radio"
                  name="notification-delivery-channel"
                  className="sr-only"
                  value={option.value}
                  checked={selected}
                  disabled={disabled}
                  onChange={() => void selectChannel(option.value)}
                />
                <span
                  className={cn(
                    "flex h-9 w-9 shrink-0 items-center justify-center rounded-lg transition-all duration-200",
                    selected
                      ? "bg-gradient-to-br from-accent to-[rgb(var(--router-accent-secondary))] text-white"
                      : "bg-muted text-muted-foreground",
                  )}
                  aria-hidden
                >
                  <Icon className="h-4 w-4" />
                </span>
                <div className="min-w-0 flex-1">
                  <div className="text-sm font-medium text-foreground">{t(option.labelKey)}</div>
                  <div className="text-xs leading-relaxed text-muted-foreground">
                    {reason && !selected ? reason : t(option.hintKey)}
                  </div>
                </div>
                {selected ? (
                  <span className="inline-flex shrink-0 items-center gap-1.5 rounded-full bg-accent px-2.5 py-1 text-xs font-medium text-white">
                    <Check className="h-3 w-3" aria-hidden />
                    {t("account.notifications.channel.selected")}
                  </span>
                ) : (
                  <span
                    className={cn(
                      "h-4 w-4 shrink-0 rounded-full border-2 transition-colors duration-200",
                      disabled ? "border-border" : "border-muted-foreground/40 group-hover:border-accent",
                    )}
                    aria-hidden
                  />
                )}
              </label>
            );
          })}
        </div>
        {fallbackActive ? (
          <p className="flex items-start gap-2 rounded-lg border border-amber-200 bg-amber-50/70 px-3 py-2 text-xs text-amber-950">
            <AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0" aria-hidden />
            {t("account.notifications.channel.fallbackNotice")}
          </p>
        ) : null}
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

        {(settings?.telegramBotFailureCode
          || settings?.telegramBotFailureHint
          || settings?.telegramBotFailureDetails
          || settings?.telegramBotTransportStatus === "degraded") ? (
          <AccountTelegramDiagnostic settings={settings} />
        ) : null}

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

function AccountTelegramDiagnostic({ settings }: { settings: NotificationSettings }) {
  const { t } = useLocaleText();
  const code = settings.telegramBotFailureCode?.trim();
  const key = code
    ? `settings.alertChannels.diagnostic.${code}`
    : "settings.alertChannels.diagnostic.legacy";
  const translated = t(key as Parameters<typeof t>[0]);
  const hint = translated === key
    ? settings.telegramBotFailureHint || t("settings.alertChannels.diagnostic.legacy")
    : translated;
  const details = settings.telegramBotFailureDetails;
  const resolved = formatDiagnosticAddresses(details?.resolvedAddresses);
  const reachable = formatDiagnosticAddresses(details?.reachableAddresses);
  const dnsError = typeof details?.dnsError === "string" ? details.dnsError : "";
  const technicalError = typeof details?.technicalError === "string" ? details.technicalError : "";
  return (
    <div className="rounded-lg border border-amber-200 bg-amber-50/70 px-3 py-2 text-xs text-amber-950">
      <p className="font-medium">{t("settings.alertChannels.diagnostic.title")}</p>
      <p className="mt-1 leading-5">{hint}</p>
      {resolved || reachable || dnsError || technicalError ? (
        <details className="mt-2 text-amber-900/80">
          <summary className="cursor-pointer select-none font-medium">
            {t("settings.alertChannels.diagnostic.details")}
          </summary>
          <div className="mt-1 grid gap-1 break-words font-mono text-[11px]">
            {resolved ? <span>{t("settings.alertChannels.diagnostic.resolved", { addresses: resolved })}</span> : null}
            {reachable ? <span>{t("settings.alertChannels.diagnostic.reachable", { addresses: reachable })}</span> : null}
            {dnsError ? <span>{t("settings.alertChannels.diagnostic.dnsError", { error: dnsError })}</span> : null}
            {technicalError ? <span>{technicalError}</span> : null}
          </div>
        </details>
      ) : null}
    </div>
  );
}

function formatDiagnosticAddresses(value: unknown) {
  if (!Array.isArray(value)) return "";
  return value.filter((item): item is string => typeof item === "string").join(", ");
}

function formatTelegramError(error: unknown, translate: ReturnType<typeof useLocaleText>["t"]) {
  if (error instanceof ApiError) {
    const code = error.details?.failureCode;
    if (typeof code === "string") {
      const key = `settings.alertChannels.diagnostic.${code}` as Parameters<typeof translate>[0];
      const translated = translate(key);
      if (translated !== key) return translated;
    }
    const hint = error.details?.failureHint;
    if (typeof hint === "string" && hint.trim()) return hint;
    if (error.code === "USER_NOTIFICATION_BOT_NOT_READY") {
      return translate("account.notifications.telegramUnavailable");
    }
  }
  return error instanceof Error ? error.message : String(error);
}
