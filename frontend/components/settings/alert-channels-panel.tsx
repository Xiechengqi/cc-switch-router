"use client";

import { Alert, Button, Chip } from "@heroui/react";
import { Bell, Loader2, RefreshCw, Send } from "lucide-react";
import * as React from "react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { alertChannelLabel } from "@/lib/alerting";
import { getAlertingChannels, testAlertingChannel } from "@/lib/api";
import type { AlertChannelState } from "@/lib/types";
import { formatDateTime } from "@/lib/utils";

export function AlertChannelsPanel({
  channel,
  refreshToken,
}: {
  channel?: string;
  refreshToken: number;
}) {
  const { t } = useLocaleText();
  const [channels, setChannels] = React.useState<AlertChannelState[]>([]);
  const [busy, setBusy] = React.useState("");
  const [error, setError] = React.useState("");
  const [success, setSuccess] = React.useState("");

  const load = React.useCallback(async () => {
    setBusy((current) => current || "load");
    try {
      setChannels(await getAlertingChannels());
      setError("");
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy("");
    }
  }, []);

  React.useEffect(() => {
    void load();
  }, [load, refreshToken]);

  const visible = channel
    ? channels.filter((item) => item.channel === channel)
    : channels;

  return (
    <section className="grid gap-3 border-b pb-5">
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

      <div className="grid gap-2">
        {visible.map((item) => {
          const testing = busy === `test:${item.channel}`;
          return (
            <div key={item.channel} className="grid gap-3 rounded-md border bg-background p-3 md:grid-cols-[minmax(0,1fr)_auto] md:items-center">
              <div className="min-w-0">
                <div className="flex flex-wrap items-center gap-2">
                  <Bell className="h-4 w-4 text-muted-foreground" />
                  <span className="font-medium">{alertChannelLabel(item.channel)}</span>
                  <ChannelStatusChip state={item} />
                </div>
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
                onClick={() => void runTest(item.channel)}
                isDisabled={!!busy || !item.configured}
              >
                {testing ? <Loader2 className="h-4 w-4 animate-spin" /> : <Send className="h-4 w-4" />}
                {t("settings.alertChannels.sendTest")}
              </Button>
            </div>
          );
        })}
        {busy === "load" && visible.length === 0 ? (
          <p className="text-sm text-muted-foreground">{t("common.loading")}</p>
        ) : null}
      </div>
    </section>
  );

  async function runTest(target: string) {
    setBusy(`test:${target}`);
    setError("");
    setSuccess("");
    try {
      await testAlertingChannel(target);
      setSuccess(t("settings.alertChannels.testSent", {
        channel: alertChannelLabel(target),
      }));
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy("");
      await load();
    }
  }
}

function ChannelStatusChip({ state }: { state: AlertChannelState }) {
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
