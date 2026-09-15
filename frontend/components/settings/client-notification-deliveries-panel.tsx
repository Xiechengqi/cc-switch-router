"use client";

import { Alert, Button, Card, Chip, Input } from "@heroui/react";
import { Loader2, RefreshCw, RotateCcw } from "lucide-react";
import * as React from "react";
import { CompactSelect } from "@/components/common/compact-select";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { getAlertingOverview, getClientChatDeliveries, getClientNotificationDeliveries, requeueClientChatDelivery } from "@/lib/api";
import type { MessageKey } from "@/lib/i18n";
import type { AlertingOverview, ClientChatDelivery, ClientNotificationDelivery } from "@/lib/types";

export function ClientNotificationDeliveriesPanel() {
  const { locale, t } = useLocaleText();
  const [deliveries, setDeliveries] = React.useState<ClientNotificationDelivery[]>([]);
  const [alerts, setAlerts] = React.useState<AlertingOverview | null>(null);
  const [source, setSource] = React.useState("all");
  const [channel, setChannel] = React.useState("all");
  const [status, setStatus] = React.useState("all");
  const [query, setQuery] = React.useState("");
  const [page, setPage] = React.useState(0);
  const [loading, setLoading] = React.useState(true);
  const [error, setError] = React.useState("");

  const load = React.useCallback(async () => {
    setLoading(true);
    setError("");
    try {
      const [response, alerting] = await Promise.allSettled([
        getClientNotificationDeliveries(),
        getAlertingOverview(10_000),
      ]);
      setDeliveries(response.status === "fulfilled" ? response.value.deliveries || [] : []);
      setAlerts(alerting.status === "fulfilled" ? alerting.value : null);
      const failures = [response, alerting]
        .filter((result): result is PromiseRejectedResult => result.status === "rejected")
        .map((result) => result.reason instanceof Error ? result.reason.message : String(result.reason));
      setError(failures.join(" · "));
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setLoading(false);
    }
  }, []);

  React.useEffect(() => {
    load().catch(console.error);
  }, [load]);

  React.useEffect(() => setPage(0), [source, channel, status, query]);

  const records = [
    ...deliveries.map((delivery) => ({
      id: `user:${delivery.id}`, source: "user", channel: delivery.channel,
      status: delivery.status, attempts: delivery.attempts,
      createdAt: delivery.createdAt, resultAt: deliveryResultTime(delivery),
      event: deliveryLabel(delivery.deliveryKind, delivery.eventKind, delivery.status, t),
      detail: delivery.eventKind, target: delivery.targetMasked,
      body: "", providerMessageId: "",
      error: delivery.errorMessage || delivery.failureKind || delivery.blockedReasonCode || "",
    })),
    ...(alerts?.deliveries || []).map((delivery) => {
      return {
        id: `operator:${delivery.id}`, source: "operator", channel: delivery.channel,
        status: delivery.status, attempts: delivery.attempts,
        createdAt: new Date(delivery.createdAt * 1000).toISOString(),
        resultAt: delivery.sentAt ? new Date(delivery.sentAt * 1000).toISOString() : null,
        event: delivery.title || t("notifications.operatorAlerts"), detail: `${delivery.eventKind} · ${delivery.transition} · ${delivery.severity}`,
        target: t("notifications.operatorTarget"), body: delivery.bodyPreview,
        providerMessageId: delivery.providerMessageId || "", error: delivery.lastError || "",
      };
    }),
  ].sort((a, b) => Date.parse(b.createdAt) - Date.parse(a.createdAt));
  const normalizedQuery = query.trim().toLowerCase();
  const filtered = records.filter((record) =>
    (source === "all" || record.source === source)
    && (channel === "all" || record.channel === channel)
    && (status === "all" || (status === "failed"
      ? ["retry", "dead_letter", "blocked_config"].includes(record.status)
      : status === "suppressed" ? record.status.startsWith("suppressed") || record.status.startsWith("cancelled") || record.status === "superseded" : record.status === status))
    && (!normalizedQuery || `${record.event} ${record.detail} ${record.target} ${record.error}`.toLowerCase().includes(normalizedQuery))
  );
  const pageSize = 50;
  const pageCount = Math.max(1, Math.ceil(filtered.length / pageSize));
  const safePage = Math.min(page, pageCount - 1);
  const visible = filtered.slice(safePage * pageSize, (safePage + 1) * pageSize);

  return (
    <div className="grid gap-6">
    <Card className="rounded-lg">
      <Card.Header className="flex-row items-start justify-between gap-4 space-y-0">
        <div>
          <Card.Title>{t("notifications.title")}</Card.Title>
          <Card.Description>{t("notifications.description")}</Card.Description>
        </div>
        <Button variant="outline" onClick={() => load()} isDisabled={loading}>
          {loading ? <Loader2 className="h-4 w-4 animate-spin" /> : <RefreshCw className="h-4 w-4" />}
          {t("common.reload")}
        </Button>
      </Card.Header>
      <Card.Content className="grid gap-4">
        {error ? <Alert status="danger" className="!text-slate-900">{error}</Alert> : null}
        <div className="grid gap-2 md:grid-cols-[minmax(220px,1fr)_160px_140px_140px]">
          <Input value={query} onChange={(event) => setQuery(event.target.value)} placeholder={t("notifications.search")} aria-label={t("notifications.search")} />
          <HistorySelect value={source} onChange={setSource} label={t("notifications.event")} options={[['all',t('notifications.allSources')],['operator',t('notifications.operatorAlerts')],['user',t('notifications.userNotifications')]]} />
          <HistorySelect value={channel} onChange={setChannel} label={t("notifications.channel")} options={[['all',t('notifications.allChannels')],['bark','Bark'],['telegram','Telegram'],['email','Email']]} />
          <HistorySelect value={status} onChange={setStatus} label={t("notifications.status")} options={[['all',t('notifications.allStatuses')],['sent',t('notifications.status.sent')],['failed',t('notifications.failedRetrying')],['suppressed',t('notifications.suppressed')]]} />
        </div>
        <div className="flex items-center justify-between text-xs text-muted-foreground">
          <span>{t("notifications.pageSummary", { count: filtered.length, page: safePage + 1, pages: pageCount })}</span>
          <div className="flex gap-2">
            <Button size="sm" variant="outline" onClick={() => setPage(Math.max(0, safePage - 1))} isDisabled={safePage === 0}>‹</Button>
            <Button size="sm" variant="outline" onClick={() => setPage(Math.min(pageCount - 1, safePage + 1))} isDisabled={safePage + 1 >= pageCount}>›</Button>
          </div>
        </div>
        <div className="overflow-x-auto rounded-lg border">
          <table className="w-full min-w-[960px] text-left text-sm">
            <thead className="bg-muted/50 text-xs text-muted-foreground">
              <tr>
                <th className="px-4 py-3 font-medium">{t("notifications.event")}</th>
                <th className="px-4 py-3 font-medium">{t("notifications.channel")}</th>
                <th className="px-4 py-3 font-medium">{t("notifications.target")}</th>
                <th className="px-4 py-3 font-medium">{t("notifications.status")}</th>
                <th className="px-4 py-3 font-medium">{t("notifications.attempts")}</th>
                <th className="px-4 py-3 font-medium">{t("notifications.created")}</th>
                <th className="px-4 py-3 font-medium">{t("notifications.result")}</th>
              </tr>
            </thead>
            <tbody className="divide-y">
              {visible.map((delivery) => (
                <tr key={delivery.id} className="align-top">
                  <td className="px-4 py-3 font-medium">
                    <div className="flex items-center gap-1">
                      <span>{delivery.event}</span>
                    </div>
                    <div className="mt-1 text-xs font-normal text-muted-foreground">{delivery.source === "operator" ? t("notifications.operatorAlerts") : t("notifications.userNotifications")}{delivery.detail ? ` · ${delivery.detail}` : ""}</div>
                    {delivery.body ? <details className="mt-2 max-w-[420px] text-xs font-normal text-muted-foreground"><summary className="cursor-pointer">{t("notifications.messagePreview")}</summary><pre className="mt-1 whitespace-pre-wrap font-sans">{delivery.body}</pre></details> : null}
                  </td>
                  <td className="px-4 py-3"><Chip size="sm" variant="soft">{channelLabel(delivery.channel, t)}</Chip></td>
                  <td className="px-4 py-3 font-mono text-xs">{delivery.target}</td>
                  <td className="px-4 py-3"><DeliveryStatus status={delivery.status} /></td>
                  <td className="px-4 py-3 tabular-nums">{delivery.attempts}</td>
                  <td className="px-4 py-3 whitespace-nowrap">{formatTime(delivery.createdAt, locale)}</td>
                  <td className="max-w-[300px] px-4 py-3">
                    <div className="whitespace-nowrap">{formatTime(delivery.resultAt, locale)}</div>
                    {delivery.error ? <div className="mt-1 break-words text-xs text-danger" title={delivery.error}>{delivery.error}</div> : null}
                    {delivery.providerMessageId ? <div className="mt-1 break-all font-mono text-[10px] text-muted-foreground">{delivery.providerMessageId}</div> : null}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
          {!loading && visible.length === 0 ? (
            <div className="px-4 py-12 text-center text-sm text-muted-foreground">{t("notifications.empty")}</div>
          ) : null}
          {loading && deliveries.length === 0 ? (
            <div className="flex items-center justify-center gap-2 px-4 py-12 text-sm text-muted-foreground">
              <Loader2 className="h-4 w-4 animate-spin" />
              {t("notifications.loading")}
            </div>
          ) : null}
        </div>
      </Card.Content>
    </Card>
    <ClientChatDeliveriesCard />
    </div>
  );
}

function HistorySelect({ value, onChange, label, options }: { value: string; onChange: (value: string) => void; label: string; options: [string, string][] }) {
  return <CompactSelect
    value={value}
    onChange={onChange}
    ariaLabel={label}
    triggerClassName="h-10 min-h-10 text-sm"
    options={options.map(([optionValue, optionLabel]) => ({ value: optionValue, label: optionLabel }))}
  />;
}

function ClientChatDeliveriesCard() {
  const { locale, t } = useLocaleText();
  const [deliveries, setDeliveries] = React.useState<ClientChatDelivery[]>([]);
  const [loading, setLoading] = React.useState(true);
  const [error, setError] = React.useState("");
  const [requeueing, setRequeueing] = React.useState<string | null>(null);

  const load = React.useCallback(async () => {
    setLoading(true);
    setError("");
    try {
      const response = await getClientChatDeliveries();
      setDeliveries(response.deliveries || []);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setLoading(false);
    }
  }, []);

  React.useEffect(() => {
    void load();
  }, [load]);

  async function requeue(delivery: ClientChatDelivery) {
    setRequeueing(delivery.id);
    setError("");
    try {
      await requeueClientChatDelivery(delivery.id);
      await load();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setRequeueing(null);
    }
  }

  return (
    <Card className="rounded-lg">
      <Card.Header className="flex-row items-start justify-between gap-4 space-y-0">
        <div>
          <Card.Title>{t("notifications.chatTitle")}</Card.Title>
          <Card.Description>{t("notifications.chatDescription")}</Card.Description>
        </div>
        <Button variant="outline" onClick={() => void load()} isDisabled={loading}>
          {loading ? <Loader2 className="h-4 w-4 animate-spin" /> : <RefreshCw className="h-4 w-4" />}
          {t("common.reload")}
        </Button>
      </Card.Header>
      <Card.Content className="grid gap-4">
        {error ? <Alert status="danger" className="!text-slate-900">{error}</Alert> : null}
        <div className="overflow-x-auto rounded-lg border">
          <table className="w-full min-w-[960px] text-left text-sm">
            <thead className="bg-muted/50 text-xs text-muted-foreground">
              <tr>
                <th className="whitespace-nowrap px-4 py-3 font-medium">{t("notifications.client")}</th>
                <th className="whitespace-nowrap px-4 py-3 font-medium">{t("notifications.recipient")}</th>
                <th className="whitespace-nowrap px-4 py-3 font-medium">{t("notifications.messages")}</th>
                <th className="whitespace-nowrap px-4 py-3 font-medium">{t("notifications.status")}</th>
                <th className="whitespace-nowrap px-4 py-3 font-medium">{t("notifications.attempts")}</th>
                <th className="whitespace-nowrap px-4 py-3 font-medium">{t("notifications.created")}</th>
                <th className="whitespace-nowrap px-4 py-3 font-medium">{t("notifications.result")}</th>
              </tr>
            </thead>
            <tbody className="divide-y">
              {deliveries.map((delivery) => (
                <tr key={delivery.id} className="align-top">
                  <td className="max-w-[220px] px-4 py-3">
                    <div className="truncate font-medium" title={delivery.clientLabel}>{delivery.clientLabel}</div>
                    <div className="mt-1 truncate font-mono text-[10px] text-muted-foreground" title={delivery.installationId}>{delivery.installationId}</div>
                  </td>
                  <td className="px-4 py-3 font-mono text-xs">{delivery.recipientMasked}</td>
                  <td className="px-4 py-3 tabular-nums">{delivery.messageCount}</td>
                  <td className="whitespace-nowrap px-4 py-3"><DeliveryStatus status={delivery.status} /></td>
                  <td className="px-4 py-3 tabular-nums">{delivery.attempts}</td>
                  <td className="px-4 py-3 whitespace-nowrap">{formatTime(delivery.createdAt, locale)}</td>
                  <td className="max-w-[260px] px-4 py-3">
                    <div className="flex items-center gap-2 whitespace-nowrap">
                      <span>{formatTime(delivery.sentAt || delivery.nextAttemptAt, locale)}</span>
                      {delivery.status === "dead_letter" ? (
                        <span title={t("notifications.requeue")}>
                        <Button
                          isIconOnly
                          size="sm"
                          variant="ghost"
                          className="rounded-md"
                          onClick={() => void requeue(delivery)}
                          isDisabled={requeueing === delivery.id}
                          aria-label={t("notifications.requeue")}
                        >
                          {requeueing === delivery.id ? <Loader2 className="h-4 w-4 animate-spin" /> : <RotateCcw className="h-4 w-4" />}
                        </Button>
                        </span>
                      ) : null}
                    </div>
                    {delivery.errorMessage ? <div className="mt-1 break-words text-xs text-danger" title={delivery.errorMessage}>{delivery.errorMessage}</div> : null}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
          {!loading && deliveries.length === 0 ? <div className="px-4 py-12 text-center text-sm text-muted-foreground">{t("notifications.empty")}</div> : null}
          {loading && deliveries.length === 0 ? <div className="flex items-center justify-center gap-2 px-4 py-12 text-sm text-muted-foreground"><Loader2 className="h-4 w-4 animate-spin" />{t("notifications.loading")}</div> : null}
        </div>
      </Card.Content>
    </Card>
  );
}

function DeliveryStatus({ status }: { status: string }) {
  const { t } = useLocaleText();
  const color = status === "sent" ? "success" : status === "dead_letter" ? "danger" : status === "retry" || status === "blocked_config" ? "warning" : "default";
  return <Chip color={color} size="sm" variant="soft" className="whitespace-nowrap">{statusLabel(status, t)}</Chip>;
}

function eventLabel(kind: string, t: ReturnType<typeof useLocaleText>["t"]) {
  if (kind === "client_registered") return t("notifications.registered");
  if (kind === "client_registration_overflow") return t("notifications.registrationOverflow");
  if (kind === "client_offline") return t("notifications.offline");
  if (kind.includes(",")) return t("notifications.mixed");
  return kind;
}

function channelLabel(channel: string, t: ReturnType<typeof useLocaleText>["t"]) {
  if (channel === "email") return t("account.notifications.channel.email");
  if (channel === "telegram") return t("account.notifications.channel.telegram");
  if (channel === "bark") return t("account.notifications.channel.bark");
  return channel;
}

function deliveryLabel(kind: string, eventKind: string, status: string, t: ReturnType<typeof useLocaleText>["t"]) {
  if (kind === "incident") return t("notifications.incident");
  if (!eventKind && status === "suppressed_config_changed") return t("notifications.configSuperseded");
  if (!eventKind) return t("notifications.unknownEvent");
  return eventLabel(eventKind, t);
}

function deliveryResultTime(delivery: ClientNotificationDelivery) {
  if (delivery.sentAt) return delivery.sentAt;
  if (["pending", "retry"].includes(delivery.status)) return delivery.nextAttemptAt;
  return null;
}

function statusLabel(status: string, t: ReturnType<typeof useLocaleText>["t"]) {
  const labels: Record<string, MessageKey> = {
    pending: "notifications.status.pending",
    claimed: "notifications.status.claimed",
    retry: "notifications.status.retry",
    sent: "notifications.status.sent",
    dead_letter: "notifications.status.deadLetter",
    cancelled_owner_changed: "notifications.status.cancelledOwnerChanged",
    cancelled_message_deleted: "notifications.status.cancelledMessageDeleted",
    cancelled_room_archived: "notifications.status.cancelledRoomArchived",
    blocked_config: "notifications.status.blocked",
    cancelled_channel_changed: "notifications.status.cancelledChannelChanged",
    suppressed_rate_limit: "notifications.status.suppressedRateLimit",
    suppressed_storm: "notifications.status.suppressedStorm",
    suppressed_disabled: "notifications.status.suppressedDisabled",
    suppressed_recipient_removed: "notifications.status.suppressedRecipient",
    suppressed_config_changed: "notifications.status.suppressedConfig",
    cancelled_recovered: "notifications.status.cancelledRecovered",
  };
  const key = labels[status];
  return key ? t(key) : status;
}

function formatTime(value: string | null | undefined, locale: string) {
  if (!value) return "-";
  const parsed = new Date(value);
  if (Number.isNaN(parsed.getTime())) return value;
  return new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "medium" }).format(parsed);
}
