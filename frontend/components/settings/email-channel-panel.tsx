"use client";

import { Alert, Button, Chip } from "@heroui/react";
import { Loader2, Mail, Send } from "lucide-react";
import * as React from "react";
import { useAuth } from "@/components/auth/auth-provider";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { ApiError, testUserNotificationChannel } from "@/lib/api";
import { formatDateTime } from "@/lib/utils";

export function EmailChannelPanel({
  refreshToken,
  apiKeyConfigured,
  fromConfigured,
  pendingRestart,
}: {
  refreshToken: number;
  apiKeyConfigured: boolean;
  fromConfigured: boolean;
  pendingRestart: boolean;
}) {
  const { t } = useLocaleText();
  const { session } = useAuth();
  const [busy, setBusy] = React.useState("");
  const [error, setError] = React.useState("");
  const [success, setSuccess] = React.useState("");
  const [lastTestedAt, setLastTestedAt] = React.useState("");
  const target = maskEmail(session?.user?.email || "");
  const configured = apiKeyConfigured && fromConfigured && !pendingRestart;

  React.useEffect(() => {
    setError("");
    setSuccess("");
  }, [refreshToken]);

  return (
    <section className="grid gap-4">
      <div>
        <h3 className="text-sm font-semibold">{t("settings.emailChannel.title")}</h3>
        <p className="mt-1 text-sm text-muted-foreground">{t("settings.emailChannel.description")}</p>
      </div>

      {error ? <Alert status="danger" className="!text-slate-900">{error}</Alert> : null}
      {success ? <Alert status="success" className="!text-slate-900">{success}</Alert> : null}

      <div className="grid gap-4 rounded-md border bg-background p-4 md:grid-cols-[minmax(0,1fr)_auto] md:items-center">
        <div className="min-w-0">
          <div className="flex flex-wrap items-center gap-2">
            <Mail className="h-4 w-4 text-muted-foreground" />
            <span className="font-medium">{t("settings.emailChannel.title")}</span>
            <Chip
              color={lastTestedAt ? "success" : configured ? "accent" : pendingRestart ? "warning" : "default"}
              size="sm"
              variant="soft"
            >
              {t(lastTestedAt
                ? "settings.alertChannels.status.healthy"
                : configured
                  ? "settings.alertChannels.status.ready"
                  : pendingRestart
                    ? "settings.alertChannels.status.reconciling"
                    : "settings.alertChannels.status.misconfigured")}
            </Chip>
          </div>
          <p className="mt-1 text-sm text-muted-foreground">
            {pendingRestart
              ? t("settings.emailChannel.pendingRestart")
              : configured
                ? t("settings.emailChannel.target", { target: target || t("settings.alertChannels.user.privateTarget") })
                : t("settings.emailChannel.notConfigured")}
          </p>
          {lastTestedAt ? (
            <p className="mt-1 text-xs text-muted-foreground">
              {t("settings.alertChannels.lastSuccess", { time: formatDateTime(lastTestedAt) })}
            </p>
          ) : (
            <p className="mt-1 text-xs text-muted-foreground">{t("settings.alertChannels.neverSucceeded")}</p>
          )}
        </div>
        <Button
          variant="outline"
          size="sm"
          onClick={() => void runTest()}
          isDisabled={!!busy || !configured}
        >
          {busy === "test" ? <Loader2 className="h-4 w-4 animate-spin" /> : <Send className="h-4 w-4" />}
          {t("settings.alertChannels.sendTest")}
        </Button>
      </div>
    </section>
  );

  async function runTest() {
    setBusy("test");
    setError("");
    setSuccess("");
    try {
      const result = await testUserNotificationChannel("email");
      const sentTarget = result.targetLabel || target || t("settings.alertChannels.user.privateTarget");
      setSuccess(t("settings.emailChannel.testSent", { target: sentTarget }));
      setLastTestedAt(result.testedAt);
    } catch (err) {
      setError(formatEmailError(err, t));
    } finally {
      setBusy("");
    }
  }
}

function maskEmail(value: string) {
  const trimmed = value.trim();
  const at = trimmed.indexOf("@");
  if (at <= 0) return trimmed;
  const local = trimmed.slice(0, at);
  const domain = trimmed.slice(at + 1);
  const visible = local.charAt(0) || "*";
  return `${visible}***@${domain}`;
}

function formatEmailError(error: unknown, translate: ReturnType<typeof useLocaleText>["t"]) {
  if (error instanceof ApiError) {
    if (error.code === "USER_NOTIFICATION_CHANNEL_MISCONFIGURED") {
      return translate("settings.emailChannel.notConfigured");
    }
    const hint = error.details?.failureHint;
    if (typeof hint === "string" && hint.trim()) return hint;
    if (error.message.trim()) return error.message;
  }
  return error instanceof Error ? error.message : String(error);
}
