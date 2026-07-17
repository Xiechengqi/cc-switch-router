"use client";

import { Alert, Button, Card, Chip } from "@heroui/react";
import { Loader2, RefreshCw, RotateCcw } from "lucide-react";
import * as React from "react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { getClientChatDeliveries, getClientNotificationDeliveries, requeueClientChatDelivery } from "@/lib/api";
import type { MessageKey } from "@/lib/i18n";
import type { ClientChatDelivery, ClientNotificationDelivery } from "@/lib/types";

export function ClientNotificationDeliveriesPanel() {
  const { locale, t } = useLocaleText();
  const [deliveries, setDeliveries] = React.useState<ClientNotificationDelivery[]>([]);
  const [loading, setLoading] = React.useState(true);
  const [error, setError] = React.useState("");

  const load = React.useCallback(async () => {
    setLoading(true);
    setError("");
    try {
      const response = await getClientNotificationDeliveries();
      setDeliveries(response.deliveries || []);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setLoading(false);
    }
  }, []);

  React.useEffect(() => {
    load().catch(console.error);
  }, [load]);

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
        <div className="overflow-x-auto rounded-lg border">
          <table className="w-full min-w-[820px] text-left text-sm">
            <thead className="bg-muted/50 text-xs text-muted-foreground">
              <tr>
                <th className="px-4 py-3 font-medium">{t("notifications.event")}</th>
                <th className="px-4 py-3 font-medium">{t("notifications.recipient")}</th>
                <th className="px-4 py-3 font-medium">{t("notifications.status")}</th>
                <th className="px-4 py-3 font-medium">{t("notifications.attempts")}</th>
                <th className="px-4 py-3 font-medium">{t("notifications.created")}</th>
                <th className="px-4 py-3 font-medium">{t("notifications.result")}</th>
              </tr>
            </thead>
            <tbody className="divide-y">
              {deliveries.map((delivery) => (
                <tr key={delivery.id} className="align-top">
                  <td className="px-4 py-3 font-medium">
                    <div className="flex items-center gap-1">
                      <span>{deliveryLabel(delivery.deliveryKind, delivery.eventKind, delivery.status, t)}</span>
                      {delivery.eventCount > 1 ? <span className="text-muted-foreground">x{delivery.eventCount}</span> : null}
                    </div>
                    {delivery.deliveryKind === "incident" ? (
                      <div className="mt-1 text-xs font-normal text-muted-foreground">{eventLabel(delivery.eventKind, t)}</div>
                    ) : null}
                  </td>
                  <td className="px-4 py-3 font-mono text-xs">{delivery.recipientMasked}</td>
                  <td className="px-4 py-3"><DeliveryStatus status={delivery.status} /></td>
                  <td className="px-4 py-3 tabular-nums">{delivery.attempts}</td>
                  <td className="px-4 py-3 whitespace-nowrap">{formatTime(delivery.createdAt, locale)}</td>
                  <td className="max-w-[300px] px-4 py-3">
                    <div className="whitespace-nowrap">{formatTime(deliveryResultTime(delivery), locale)}</div>
                    {delivery.errorMessage ? <div className="mt-1 break-words text-xs text-danger" title={delivery.errorMessage}>{delivery.errorMessage}</div> : null}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
          {!loading && deliveries.length === 0 ? (
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
