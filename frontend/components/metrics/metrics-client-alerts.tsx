"use client";

import { Alert, Button, Card, Chip, Input, ScrollShadow } from "@heroui/react";
import {
  BellOff,
  BellRing,
  Check,
  CircleAlert,
  Clock3,
  Loader2,
  MonitorCheck,
  MonitorX,
  RefreshCw,
  Users,
} from "lucide-react";
import * as React from "react";
import { CompactSelect } from "@/components/common/compact-select";
import { useLocaleText } from "@/components/i18n/locale-provider";
import {
  acknowledgeAlertIncident,
  resumeAlertIncident,
  retryAlertDelivery,
  silenceAlertIncident,
} from "@/lib/api";
import { alertChannelLabel } from "@/lib/alerting";
import type { MessageKey } from "@/lib/i18n";
import type {
  AlertChannelState,
  AlertDelivery,
  AlertIncident,
  AlertingOverview,
  ClientMetricsSnapshot,
  MetricsSeriesResponse,
} from "@/lib/types";
import { formatDateTime, formatNumber } from "@/lib/utils";
import { KpiGrid, MetricKpiCard } from "./metrics-cards";
import { type ChartState, ResourceTrendChart } from "./metrics-charts";
import { SimpleTable } from "./metrics-tables";

export function ClientsTab({
  clients,
  series,
  state,
}: {
  clients?: ClientMetricsSnapshot;
  series: MetricsSeriesResponse | null;
  state: ChartState;
}) {
  const { t } = useLocaleText();
  const points = series?.clients || [];
  return (
    <div className="grid animate-fade-in-up gap-5">
      <KpiGrid>
        <MetricKpiCard
          label={t("metrics.clients.total")}
          value={formatNumber(clients?.total)}
          detail={t("metrics.clients.monitored", { count: formatNumber(clients?.monitored) })}
          icon={<Users />}
          spark={points.map((point) => point.total)}
        />
        <MetricKpiCard
          label={t("metrics.clients.online")}
          value={formatNumber(clients?.online)}
          detail={t("metrics.clients.onlineDesc")}
          icon={<MonitorCheck />}
          tone="default"
          spark={points.map((point) => point.online)}
        />
        <MetricKpiCard
          label={t("metrics.clients.offline")}
          value={formatNumber(clients?.offline)}
          detail={t("metrics.clients.offlineDesc")}
          icon={<MonitorX />}
          tone={(clients?.offline || 0) > 0 ? "critical" : "default"}
          spark={points.map((point) => point.offline)}
        />
        <MetricKpiCard
          label={t("metrics.clients.recovering")}
          value={formatNumber(clients?.recovering)}
          detail={t("metrics.clients.recoveringDesc")}
          icon={<RefreshCw />}
          tone={(clients?.recovering || 0) > 0 ? "warning" : "default"}
          spark={points.map((point) => point.recovering)}
        />
      </KpiGrid>

      <ResourceTrendChart
        title={t("metrics.clients.trend")}
        maxMode="auto"
        state={state}
        series={[
          { label: t("metrics.clients.online"), color: "#10B981", values: points.map((point) => point.online) },
          { label: t("metrics.clients.recovering"), color: "#F59E0B", values: points.map((point) => point.recovering) },
          { label: t("metrics.clients.offline"), color: "#EF4444", values: points.map((point) => point.offline) },
          { label: t("metrics.clients.unknown"), color: "#64748B", values: points.map((point) => point.unknown) },
        ]}
        timestamps={points.map((point) => point.timestamp)}
      />

      <Card className="rounded-xl">
        <Card.Header>
          <Card.Title>{t("metrics.clients.inventory")}</Card.Title>
          <Card.Description>{t("metrics.clients.inventoryDesc")}</Card.Description>
        </Card.Header>
        <Card.Content>
          <SimpleTable
            headers={[
              t("metrics.clients.client"),
              t("metrics.clients.status"),
              t("metrics.clients.platform"),
              t("metrics.clients.lastSeen"),
              t("metrics.clients.offlineSince"),
              t("metrics.clients.episode"),
            ]}
            rows={(clients?.items || []).map((item) => [
              <div key="client" className="min-w-[160px]">
                <div className="font-medium">{item.clientLabel}</div>
                <div className="font-mono text-[11px] text-muted-foreground">{item.installationId}</div>
              </div>,
              <ClientStatusChip key="status" status={item.status} monitoringEnabled={item.monitoringEnabled} />,
              <div key="platform" className="min-w-[110px]">
                <div>{item.platform || "-"}</div>
                <div className="text-xs text-muted-foreground">{item.appVersion || "-"}</div>
              </div>,
              item.lastAuthenticatedSeenAt ? formatDateTime(item.lastAuthenticatedSeenAt * 1000) : "-",
              item.offlineSince ? formatDateTime(item.offlineSince * 1000) : "-",
              formatNumber(item.offlineEpisode),
            ])}
          />
        </Card.Content>
      </Card>
    </div>
  );
}

export function AlertsTab({
  overview,
  onReload,
}: {
  overview: AlertingOverview | null;
  onReload: () => Promise<void>;
}) {
  const { t } = useLocaleText();
  const [note, setNote] = React.useState("");
  const [silenceDuration, setSilenceDuration] = React.useState("3600");
  const [busy, setBusy] = React.useState("");
  const [error, setError] = React.useState("");
  const [success, setSuccess] = React.useState("");
  const active = (overview?.incidents || []).filter((incident) => !incident.resolvedAt);
  const failedDeliveryCount = overview?.failedDeliveryCount || 0;

  return (
    <div className="grid animate-fade-in-up gap-5">
      {error ? <Alert status="danger" className="!text-slate-900">{error}</Alert> : null}
      {success ? <Alert status="success" className="!text-slate-900">{success}</Alert> : null}

      <KpiGrid>
        <MetricKpiCard
          label={t("metrics.alerts.active")}
          value={formatNumber(overview?.activeCount)}
          detail={t("metrics.alerts.activeDesc")}
          icon={<CircleAlert />}
          tone={(overview?.activeCount || 0) > 0 ? "warning" : "default"}
        />
        <MetricKpiCard
          label={t("metrics.alerts.critical")}
          value={formatNumber(overview?.criticalCount)}
          detail={t("metrics.alerts.criticalDesc")}
          icon={<MonitorX />}
          tone={(overview?.criticalCount || 0) > 0 ? "critical" : "default"}
        />
        <MetricKpiCard
          label={t("metrics.alerts.failedDeliveries")}
          value={formatNumber(overview?.failedDeliveryCount)}
          detail={t("metrics.alerts.failedDeliveriesDesc")}
          icon={<RefreshCw />}
          tone={failedDeliveryCount > 0 ? "warning" : "default"}
        />
        <MetricKpiCard
          label={t("metrics.alerts.resolved")}
          value={formatNumber(overview?.resolvedCount)}
          detail={t("metrics.alerts.historyDesc")}
          icon={<Check />}
        />
      </KpiGrid>

      <Card className="rounded-xl">
        <Card.Header>
          <Card.Title>{t("metrics.alerts.channels")}</Card.Title>
          <Card.Description>{t("metrics.alerts.channelsDesc")}</Card.Description>
        </Card.Header>
        <Card.Content className="grid gap-2 md:grid-cols-2">
          {(overview?.channels || []).map((channel) => <ChannelRow key={channel.channel} channel={channel} />)}
        </Card.Content>
      </Card>

      <Card className="rounded-xl">
        <Card.Header>
          <Card.Title>{t("metrics.alerts.incidents")}</Card.Title>
          <Card.Description>{t("metrics.alerts.incidentsDesc")}</Card.Description>
        </Card.Header>
        <Card.Content className="grid gap-4">
          <div className="grid gap-3 border-b pb-4 md:grid-cols-[minmax(0,1fr)_180px]">
            <Input
              value={note}
              onChange={(event) => setNote(event.target.value)}
              placeholder={t("metrics.alerts.notePlaceholder")}
              aria-label={t("metrics.alerts.notePlaceholder")}
            />
            <CompactSelect
              value={silenceDuration}
              onChange={setSilenceDuration}
              ariaLabel={t("metrics.alerts.silenceDuration")}
              options={[
                { value: "3600", label: t("metrics.alerts.silence1h") },
                { value: "21600", label: t("metrics.alerts.silence6h") },
                { value: "86400", label: t("metrics.alerts.silence24h") },
                { value: "604800", label: t("metrics.alerts.silence7d") },
              ]}
            />
          </div>
          <ScrollShadow className="max-h-[520px]">
            {active.length === 0 ? (
              <p className="py-3 text-sm text-muted-foreground">{t("metrics.alerts.noActive")}</p>
            ) : (
              <div className="grid gap-2 pr-2">
                {active.map((incident) => (
                  <IncidentRow
                    key={incident.id}
                    incident={incident}
                    busy={busy}
                    silenceDuration={Number(silenceDuration)}
                    onAction={(action) => void runIncidentAction(incident, action)}
                  />
                ))}
              </div>
            )}
          </ScrollShadow>
        </Card.Content>
      </Card>

      <Card className="rounded-xl">
        <Card.Header>
          <Card.Title>{t("metrics.alerts.deliveries")}</Card.Title>
          <Card.Description>{t("metrics.alerts.deliveriesDesc")}</Card.Description>
        </Card.Header>
        <Card.Content>
          <SimpleTable
            headers={[
              t("metrics.alerts.channel"),
              t("metrics.alerts.deliveryStatus"),
              t("metrics.alerts.attempts"),
              t("metrics.alerts.updated"),
              t("metrics.alerts.error"),
              t("common.actions"),
            ]}
            rows={(overview?.deliveries || []).map((delivery) => [
              alertChannelLabel(delivery.channel),
              <DeliveryStatusChip key="status" delivery={delivery} />,
              formatNumber(delivery.attempts),
              formatDateTime(delivery.updatedAt * 1000),
              <span key="error" className="block max-w-[320px] break-words text-xs text-muted-foreground">{delivery.lastError || "-"}</span>,
              <Button
                key="retry"
                variant="ghost"
                size="sm"
                onClick={() => void runDeliveryRetry(delivery)}
                isDisabled={!!busy || !["retry", "dead_letter", "suppressed_disabled"].includes(delivery.status)}
              >
                {busy === `delivery:${delivery.id}` ? <Loader2 className="h-4 w-4 animate-spin" /> : <RefreshCw className="h-4 w-4" />}
                {t("common.retry")}
              </Button>,
            ])}
          />
        </Card.Content>
      </Card>
    </div>
  );

  async function runIncidentAction(incident: AlertIncident, action: "acknowledge" | "silence" | "resume") {
    setBusy(`incident:${incident.id}:${action}`);
    setError("");
    setSuccess("");
    try {
      if (action === "acknowledge") await acknowledgeAlertIncident(incident.id, note);
      if (action === "silence") await silenceAlertIncident(incident.id, Number(silenceDuration), note);
      if (action === "resume") await resumeAlertIncident(incident.id, note);
      setSuccess(t(`metrics.alerts.action.${action}.success` as MessageKey));
      setNote("");
      await onReload();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy("");
    }
  }

  async function runDeliveryRetry(delivery: AlertDelivery) {
    setBusy(`delivery:${delivery.id}`);
    setError("");
    setSuccess("");
    try {
      await retryAlertDelivery(delivery.id);
      setSuccess(t("metrics.alerts.retryQueued"));
      await onReload();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy("");
    }
  }
}

function ClientStatusChip({ status, monitoringEnabled }: { status: string; monitoringEnabled: boolean }) {
  const { t } = useLocaleText();
  const color = status === "online" ? "success" : status === "recovering" ? "warning" : status === "offline" ? "danger" : "default";
  const label = monitoringEnabled ? t(`metrics.clients.status.${status}` as MessageKey) : t("metrics.clients.notMonitored");
  return <Chip color={color} size="sm" variant="soft">{label}</Chip>;
}

function ChannelRow({ channel }: { channel: AlertChannelState }) {
  const { t } = useLocaleText();
  return (
    <div className="rounded-md border bg-background p-3">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <span className="font-medium">{alertChannelLabel(channel.channel)}</span>
        <Chip color={channel.status === "healthy" ? "success" : channel.status === "degraded" || channel.status === "misconfigured" ? "danger" : "default"} size="sm" variant="soft">
          {t(`settings.alertChannels.status.${channel.status}` as MessageKey)}
        </Chip>
      </div>
      <p className="mt-2 text-xs text-muted-foreground">
        {channel.lastSuccessAt ? formatDateTime(channel.lastSuccessAt * 1000) : t("metrics.alerts.neverDelivered")}
      </p>
      {channel.lastError ? <p className="mt-1 break-words text-xs text-red-600">{channel.lastError}</p> : null}
    </div>
  );
}

function IncidentRow({
  incident,
  busy,
  onAction,
}: {
  incident: AlertIncident;
  busy: string;
  silenceDuration: number;
  onAction: (action: "acknowledge" | "silence" | "resume") => void;
}) {
  const { t } = useLocaleText();
  const isBusy = busy.startsWith(`incident:${incident.id}:`);
  return (
    <div className="grid gap-3 rounded-md border bg-background p-3 lg:grid-cols-[minmax(0,1fr)_auto] lg:items-center">
      <div className="min-w-0">
        <div className="flex flex-wrap items-center gap-2">
          <Chip color={incident.severity === "critical" ? "danger" : incident.severity === "warning" ? "warning" : "default"} size="sm" variant="soft">
            {t(`metrics.alerts.severity.${incident.severity}` as MessageKey)}
          </Chip>
          <Chip color={incident.status === "firing" ? "danger" : incident.status === "silenced" ? "warning" : "accent"} size="sm" variant="soft">
            {t(`metrics.alerts.status.${incident.status}` as MessageKey)}
          </Chip>
          <span className="font-medium">{incident.title}</span>
        </div>
        <p className="mt-1 text-sm">{incident.message}</p>
        <p className="mt-1 text-xs text-muted-foreground">
          {incident.entityId ? `${incident.entityKind}: ${incident.entityId} · ` : ""}
          {t("metrics.alerts.started", { time: formatDateTime(incident.startedAt * 1000) })}
          {` · ${t("metrics.alerts.occurrences", { count: incident.occurrenceCount })}`}
        </p>
        {incident.silencedUntil ? (
          <p className="mt-1 flex items-center gap-1 text-xs text-amber-700">
            <Clock3 className="h-3.5 w-3.5" />
            {t("metrics.alerts.silencedUntil", { time: formatDateTime(incident.silencedUntil * 1000) })}
          </p>
        ) : null}
      </div>
      <div className="flex flex-wrap gap-2 lg:justify-end">
        {incident.status === "firing" ? (
          <Button variant="outline" size="sm" onClick={() => onAction("acknowledge")} isDisabled={isBusy}>
            {busy.endsWith(":acknowledge") ? <Loader2 className="h-4 w-4 animate-spin" /> : <Check className="h-4 w-4" />}
            {t("metrics.alerts.acknowledge")}
          </Button>
        ) : null}
        {incident.status !== "silenced" ? (
          <Button variant="outline" size="sm" onClick={() => onAction("silence")} isDisabled={isBusy}>
            {busy.endsWith(":silence") ? <Loader2 className="h-4 w-4 animate-spin" /> : <BellOff className="h-4 w-4" />}
            {t("metrics.alerts.silence")}
          </Button>
        ) : null}
        {incident.status === "acknowledged" || incident.status === "silenced" ? (
          <Button variant="outline" size="sm" onClick={() => onAction("resume")} isDisabled={isBusy}>
            {busy.endsWith(":resume") ? <Loader2 className="h-4 w-4 animate-spin" /> : <BellRing className="h-4 w-4" />}
            {t("metrics.alerts.resume")}
          </Button>
        ) : null}
      </div>
    </div>
  );
}

function DeliveryStatusChip({ delivery }: { delivery: AlertDelivery }) {
  const { t } = useLocaleText();
  const color = delivery.status === "sent" ? "success" : delivery.status === "retry" || delivery.status === "claimed" || delivery.status === "pending" ? "warning" : delivery.status === "dead_letter" ? "danger" : "default";
  return <Chip color={color} size="sm" variant="soft">{t(`metrics.alerts.delivery.${delivery.status}` as MessageKey)}</Chip>;
}
