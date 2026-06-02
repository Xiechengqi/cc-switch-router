"use client";

import {
  Activity,
  AlertTriangle,
  BrainCircuit,
  Cpu,
  Gauge,
  HardDrive,
  Loader2,
  MemoryStick,
  Network,
  RefreshCw,
  Route,
  ServerCrash,
  Shuffle,
  Trash2,
} from "lucide-react";
import { Alert, Button, Card, Switch } from "@heroui/react";
import * as React from "react";
import { ConfirmAlertDialog } from "@/components/common/confirm-alert-dialog";
import { useAuth } from "@/components/auth/auth-provider";
import { useLocaleText } from "@/components/i18n/locale-provider";
import {
  clearMetrics,
  getLlmMetricsFailover,
  getLlmMetricsTop,
  getMetricEvents,
  getMetricsHostInfo,
  getMetricsSeries,
  getMetricsSnapshot,
} from "@/lib/api";
import type { MessageKey } from "@/lib/i18n";
import type {
  HostMetricsInfo,
  LlmReliabilityResponse,
  LlmTopResponse,
  MetricEvent,
  MetricsSeriesResponse,
  MetricsSnapshot,
} from "@/lib/types";
import { compactTokens, fixed, formatBytes, formatDateTime, formatNumber, formatUptime, percent } from "@/lib/utils";
import { deltaSeries, diskPercent, memoryPercent, mergeMetricEvents, pipelineState, toneFor } from "./metrics-utils";
import {
  DiskUsageList,
  HostInfoPanel,
  KpiGrid,
  LiveSummaryStrip,
  MetricKpiCard,
  ProcessPanel,
  StatusChip,
} from "./metrics-cards";
import { type ChartState, ResourceTrendChart } from "./metrics-charts";
import { CountersTable, MetricEventsList, ModelSubstitutionPanel, TopConsumersTable } from "./metrics-tables";

type MetricsTab = "overview" | "host" | "router" | "llm" | "events";

const RANGES = ["15m", "1h", "6h", "24h", "7d"];

// Mirrors the backend `default_step_label` so the header can show the bucket
// size the server will pick for a given range.
function defaultStepLabel(range: string): string {
  const map: Record<string, string> = {
    "15m": "15s",
    "1h": "30s",
    "6h": "1m",
    "24h": "5m",
    "7d": "15m",
  };
  return map[range] || "30s";
}

const TAB_LABELS: Record<MetricsTab, MessageKey> = {
  overview: "metrics.tab.overview",
  host: "metrics.tab.host",
  router: "metrics.tab.router",
  llm: "metrics.tab.llm",
  events: "metrics.tab.events",
};

export function MetricsPage() {
  const { session, loading } = useAuth();
  const { t } = useLocaleText();
  const [activeTab, setActiveTab] = React.useState<MetricsTab>("overview");
  const [range, setRange] = React.useState("1h");
  const [autoRefresh, setAutoRefresh] = React.useState(true);
  const [snapshot, setSnapshot] = React.useState<MetricsSnapshot | null>(null);
  const [series, setSeries] = React.useState<MetricsSeriesResponse | null>(null);
  const [hostInfo, setHostInfo] = React.useState<HostMetricsInfo | null>(null);
  const [events, setEvents] = React.useState<MetricEvent[]>([]);
  const [top, setTop] = React.useState<LlmTopResponse | null>(null);
  const [failover, setFailover] = React.useState<LlmReliabilityResponse | null>(null);
  const [busy, setBusy] = React.useState("");
  const [error, setError] = React.useState("");
  const [clearOpen, setClearOpen] = React.useState(false);
  const [banner, setBanner] = React.useState("");

  const isAdmin = !!session?.isAdmin;

  const load = React.useCallback(async (silent = false) => {
    if (!isAdmin) return;
    if (!silent) {
      setBusy((value) => value || "load");
      setError("");
    }
    try {
      const wantSeries = activeTab !== "events";
      const wantInfo = activeTab === "overview" || activeTab === "host";
      const wantEvents = activeTab === "overview" || activeTab === "events";
      const wantTop = activeTab === "llm";
      const [nextSnapshot, nextSeries, nextInfo, nextEvents, nextTop, nextFailover] = await Promise.all([
        getMetricsSnapshot(),
        wantSeries ? getMetricsSeries(range) : Promise.resolve(null),
        wantInfo ? getMetricsHostInfo() : Promise.resolve(null),
        wantEvents ? getMetricEvents(100) : Promise.resolve(null),
        wantTop ? getLlmMetricsTop(range, "tokens") : Promise.resolve(null),
        wantTop ? getLlmMetricsFailover(range, 10) : Promise.resolve(null),
      ]);
      setSnapshot(nextSnapshot);
      if (nextSeries) setSeries(nextSeries);
      if (nextInfo) setHostInfo(nextInfo);
      if (nextEvents) setEvents(nextEvents);
      if (nextTop) setTop(nextTop);
      if (nextFailover) setFailover(nextFailover);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      if (!silent) setBusy("");
    }
  }, [isAdmin, range, activeTab]);

  React.useEffect(() => {
    load().catch(console.error);
  }, [load]);

  React.useEffect(() => {
    if (!autoRefresh || !isAdmin) return;
    let cancelled = false;
    let timer = 0;
    const tick = async () => {
      await load(true).catch(console.error);
      if (!cancelled) timer = window.setTimeout(tick, 5000);
    };
    timer = window.setTimeout(tick, 5000);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [autoRefresh, isAdmin, load]);

  React.useEffect(() => {
    if (!banner) return;
    const id = window.setTimeout(() => setBanner(""), 5000);
    return () => window.clearTimeout(id);
  }, [banner]);

  if (loading) {
    return (
      <main className="mx-auto w-[calc(100%-2rem)] max-w-7xl py-12 text-muted-foreground">
        {t("common.loadingSession")}
      </main>
    );
  }

  if (!isAdmin) {
    return (
      <main className="settings-surface mx-auto grid w-[calc(100%-2rem)] max-w-4xl gap-5 py-12 text-foreground">
        <div>
          <div className="section-label">{t("metrics.title")}</div>
          <h1 className="mt-4 font-display text-4xl">{t("settings.adminRequired")}</h1>
          <p className="mt-3 text-muted-foreground">{t("settings.adminRequiredDesc")}</p>
        </div>
      </main>
    );
  }

  const lastSample = snapshot?.sampledAt ? formatDateTime(snapshot.sampledAt * 1000) : "--";
  const showSkeleton = snapshot === null && busy === "load";
  const alertEvents = mergeMetricEvents(snapshot?.alerts, events);
  const chartState = pipelineState(snapshot, busy === "load");

  return (
    <main className="settings-surface mx-auto grid w-[calc(100%-2rem)] max-w-7xl gap-5 pb-10 text-foreground">
      <section className="flex flex-wrap items-start justify-between gap-4">
        <div>
          <div className="flex flex-wrap items-center gap-2">
            <StatusChip status={snapshot?.status || "healthy"} />
            <h1 className="font-display text-3xl">
              <span className="gradient-text">{t("metrics.title")}</span>
            </h1>
            {chartState === "disabled" ? (
              <span className="rounded-full border border-amber-300 bg-amber-50 px-2.5 py-0.5 text-xs text-amber-700">
                {t("metrics.chart.disabled")}
              </span>
            ) : chartState === "stale" ? (
              <span className="rounded-full border border-amber-300 bg-amber-50 px-2.5 py-0.5 text-xs text-amber-700">
                {t("metrics.chart.stale")}
              </span>
            ) : null}
          </div>
          <p className="mt-2 text-sm text-muted-foreground">{t("metrics.subtitle")}</p>
          <p className="mt-1 text-xs text-muted-foreground">
            {t("metrics.lastSample")}: {lastSample}
            <span className="ml-2 font-mono text-[10px] uppercase tracking-[0.12em] text-muted-foreground/70">
              {range} · {defaultStepLabel(range)} bucket
            </span>
          </p>
        </div>
        <div className="flex flex-wrap items-center justify-end gap-2">
          <div className="flex rounded-xl border bg-card p-1">
            {RANGES.map((item) => (
              <button
                key={item}
                onClick={() => setRange(item)}
                className={`h-8 rounded-md px-3 text-xs transition-colors ${range === item ? "bg-primary text-primary-foreground" : "text-muted-foreground hover:text-foreground"}`}
              >
                {item}
              </button>
            ))}
          </div>
          <div className="flex h-10 items-center gap-2 rounded-xl border bg-card px-3 text-xs text-muted-foreground">
            <Switch isSelected={autoRefresh} onChange={setAutoRefresh} />
            {t("metrics.autoRefresh")}
          </div>
          <Button variant="outline" onClick={() => load()} isDisabled={!!busy}>
            {busy === "load" ? <Loader2 className="h-4 w-4 animate-spin" /> : <RefreshCw className="h-4 w-4" />}
            {t("common.reload")}
          </Button>
          <Button variant="ghost" className="!text-red-600 hover:!bg-red-50" onClick={() => setClearOpen(true)} isDisabled={!!busy}>
            <Trash2 className="h-4 w-4" />
            {t("metrics.clear")}
          </Button>
        </div>
      </section>

      {error ? <Alert status="danger" className="!text-slate-900">{error}</Alert> : null}
      {banner ? <Alert status="success" className="!text-slate-900">{banner}</Alert> : null}

      <nav className="flex gap-1 overflow-x-auto rounded-xl border bg-card p-1">
        {(["overview", "host", "router", "llm", "events"] as MetricsTab[]).map((tab) => (
          <button
            key={tab}
            onClick={() => setActiveTab(tab)}
            className={`h-9 rounded-md px-4 text-sm transition-colors ${activeTab === tab ? "bg-primary text-primary-foreground" : "text-muted-foreground hover:text-foreground"}`}
          >
            {t(TAB_LABELS[tab])}
          </button>
        ))}
      </nav>

      {showSkeleton ? (
        <MetricsSkeleton />
      ) : (
        <>
          {activeTab === "overview" ? <OverviewTab snapshot={snapshot} series={series} events={alertEvents} state={chartState} /> : null}
          {activeTab === "host" ? <HostTab snapshot={snapshot} series={series} hostInfo={hostInfo} state={chartState} /> : null}
          {activeTab === "router" ? <RouterTab snapshot={snapshot} series={series} state={chartState} /> : null}
          {activeTab === "llm" ? <LlmTab snapshot={snapshot} series={series} top={top} failover={failover} state={chartState} /> : null}
          {activeTab === "events" ? <EventsTab events={alertEvents} /> : null}
        </>
      )}

      <ConfirmAlertDialog
        open={clearOpen}
        title={t("metrics.clearTitle")}
        description={t("metrics.clearDesc")}
        confirmLabel={t("metrics.clear")}
        cancelLabel={t("common.cancel")}
        tone="danger"
        busy={busy === "clear"}
        onOpenChange={setClearOpen}
        onConfirm={async () => {
          setBusy("clear");
          setError("");
          try {
            await clearMetrics();
            setClearOpen(false);
            setBanner(t("metrics.cleared"));
            await load();
          } catch (err) {
            setError(err instanceof Error ? err.message : String(err));
          } finally {
            setBusy("");
          }
        }}
      />
    </main>
  );
}

function MetricsSkeleton() {
  return (
    <div className="grid grid-cols-1 gap-4 sm:grid-cols-2 xl:grid-cols-4">
      {Array.from({ length: 8 }).map((_, index) => (
        <Card key={index} className="rounded-xl">
          <Card.Content className="p-5">
            <div className="h-4 w-20 animate-pulse rounded bg-muted" />
            <div className="mt-3 h-7 w-24 animate-pulse rounded bg-muted" />
            <div className="mt-2 h-3 w-16 animate-pulse rounded bg-muted" />
          </Card.Content>
        </Card>
      ))}
    </div>
  );
}

function OverviewTab({
  snapshot,
  series,
  events,
  state,
}: {
  snapshot: MetricsSnapshot | null;
  series: MetricsSeriesResponse | null;
  events: MetricEvent[];
  state: ChartState;
}) {
  const { t } = useLocaleText();
  const host = snapshot?.host;
  const router = snapshot?.router;
  const llm = snapshot?.llm;
  const disk = host?.disks?.[0];
  const hostPts = series?.host || [];
  const routerPts = series?.router || [];
  const llmPts = series?.llm || [];
  return (
    <div className="grid animate-fade-in-up gap-5">
      <LiveSummaryStrip snapshot={snapshot} />
      <KpiGrid>
        <MetricKpiCard
          label={t("metrics.kpi.fdUsage")}
          value={percent(host?.process.fdUsagePercent)}
          detail={t("metrics.detail.open", {
            used: formatNumber(host?.process.openFds),
            total: formatNumber(host?.process.maxFds),
          })}
          icon={<Gauge />}
          tone={toneFor(host?.process.fdUsagePercent, 70, 85)}
          spark={hostPts.map((p) => p.fdUsagePercent || 0)}
        />
        <MetricKpiCard
          label={t("metrics.kpi.cpu")}
          value={percent(host?.cpuPercent)}
          detail={t("metrics.detail.load", { value: fixed(host?.load1) })}
          icon={<Cpu />}
          tone={toneFor(host?.cpuPercent, 75, 90)}
          spark={hostPts.map((p) => p.cpuPercent || 0)}
        />
        <MetricKpiCard
          label={t("metrics.kpi.memory")}
          value={percent(memoryPercent(host))}
          detail={`${formatBytes(host?.memoryUsedBytes)} / ${formatBytes(host?.memoryTotalBytes)}`}
          icon={<MemoryStick />}
          tone={toneFor(memoryPercent(host), 80, 92)}
          spark={hostPts.map((p) => p.memoryUsagePercent || 0)}
        />
        <MetricKpiCard
          label={t("metrics.kpi.disk")}
          value={percent(diskPercent(disk))}
          detail={`${formatBytes(disk?.usedBytes)} / ${formatBytes(disk?.totalBytes)}`}
          icon={<HardDrive />}
          tone={toneFor(diskPercent(disk), 80, 90)}
          spark={hostPts.map((p) => p.diskUsagePercent || 0)}
        />
        <MetricKpiCard
          label={t("metrics.kpi.activeRoutes")}
          value={formatNumber(router?.activeRoutes)}
          detail={t("metrics.detail.pending", { count: formatNumber(router?.pendingRoutes) })}
          icon={<Route />}
          spark={routerPts.map((p) => p.activeRoutes)}
        />
        <MetricKpiCard
          label={t("metrics.kpi.forwardListeners")}
          value={formatNumber(router?.sshForwardListeners)}
          detail={t("metrics.detail.sshSessions", { count: formatNumber(router?.sshActiveSessions) })}
          icon={<Activity />}
          tone={(router?.sshForwardListeners || 0) > (router?.activeRoutes || 0) + 2 ? "critical" : "default"}
          spark={routerPts.map((p) => p.forwardListeners)}
        />
        <MetricKpiCard
          label={t("metrics.kpi.proxyErrors")}
          value={formatNumber(router?.proxyUpstreamErrorsTotal)}
          detail={t("metrics.detail.inflight", { count: formatNumber(router?.proxyInflight) })}
          icon={<ServerCrash />}
          tone={(router?.proxyUpstreamErrorsTotal || 0) > 0 ? "warning" : "default"}
          spark={deltaSeries(routerPts.map((p) => p.proxyUpstreamErrorsTotal))}
        />
        <MetricKpiCard
          label={t("metrics.kpi.llmRpmTpm")}
          value={`${fixed(llm?.rpm)} / ${compactTokens(llm?.tpm)}`}
          detail={t("metrics.detail.error", { value: percent((llm?.errorRate || 0) * 100) })}
          icon={<BrainCircuit />}
          tone={(llm?.errorRate || 0) > 0.1 ? "warning" : "default"}
          spark={llmPts.map((p) => p.rpm)}
        />
      </KpiGrid>
      <div className="grid gap-5 xl:grid-cols-[2fr_1fr]">
        <ResourceTrendChart
          title={t("metrics.chart.systemRisk")}
          state={state}
          unit="%"
          series={[
            { label: "FD", color: "#EF4444", values: series?.host.map((p) => p.fdUsagePercent || 0) || [] },
            { label: "CPU", color: "#6366F1", values: series?.host.map((p) => p.cpuPercent || 0) || [] },
            { label: "Memory", color: "#10B981", values: series?.host.map((p) => p.memoryUsagePercent || 0) || [] },
            { label: "Disk", color: "#F59E0B", values: series?.host.map((p) => p.diskUsagePercent || 0) || [] },
          ]}
          timestamps={series?.host.map((p) => p.timestamp) || []}
        />
        <MetricEventsList events={events.slice(0, 6)} />
      </div>
    </div>
  );
}

function HostTab({
  snapshot,
  series,
  hostInfo,
  state,
}: {
  snapshot: MetricsSnapshot | null;
  series: MetricsSeriesResponse | null;
  hostInfo: HostMetricsInfo | null;
  state: ChartState;
}) {
  const { t } = useLocaleText();
  const host = snapshot?.host;
  const hostPts = series?.host || [];
  return (
    <div className="grid animate-fade-in-up gap-5">
      <KpiGrid>
        <MetricKpiCard
          label={t("metrics.kpi.uptime")}
          value={formatUptime(host?.uptimeSecs)}
          detail={t("metrics.detail.hostUptime")}
          icon={<Activity />}
        />
        <MetricKpiCard
          label={t("metrics.kpi.cpu")}
          value={percent(host?.cpuPercent)}
          detail={t("metrics.detail.cores", { count: hostInfo?.cpuCores ?? "-" })}
          icon={<Cpu />}
          tone={toneFor(host?.cpuPercent, 75, 90)}
          spark={hostPts.map((p) => p.cpuPercent || 0)}
        />
        <MetricKpiCard
          label={t("metrics.kpi.memory")}
          value={formatBytes(host?.memoryUsedBytes)}
          detail={t("metrics.detail.total", { value: formatBytes(host?.memoryTotalBytes) })}
          icon={<MemoryStick />}
          spark={hostPts.map((p) => p.memoryUsagePercent || 0)}
        />
        <MetricKpiCard
          label={t("metrics.kpi.network")}
          value={`${formatBytes(host?.network.rxBytesPerSec)}/s`}
          detail={t("metrics.detail.txRate", { value: formatBytes(host?.network.txBytesPerSec) })}
          icon={<Network />}
          spark={hostPts.map((p) => p.rxBytesPerSec || 0)}
        />
      </KpiGrid>
      <div className="grid gap-5 xl:grid-cols-[2fr_1fr]">
        <ResourceTrendChart
          title={t("metrics.chart.hostPerformance")}
          state={state}
          unit="%"
          series={[
            { label: "CPU", color: "#6366F1", values: series?.host.map((p) => p.cpuPercent || 0) || [] },
            { label: "Memory", color: "#10B981", values: series?.host.map((p) => p.memoryUsagePercent || 0) || [] },
            { label: "Disk", color: "#F59E0B", values: series?.host.map((p) => p.diskUsagePercent || 0) || [] },
            { label: "FD", color: "#EF4444", values: series?.host.map((p) => p.fdUsagePercent || 0) || [] },
          ]}
          timestamps={series?.host.map((p) => p.timestamp) || []}
        />
        <ProcessPanel host={host} />
      </div>
      <div className="grid gap-5 xl:grid-cols-[1.3fr_1fr]">
        <DiskUsageList disks={host?.disks || []} />
        <HostInfoPanel info={hostInfo} host={host} />
      </div>
    </div>
  );
}

function RouterTab({
  snapshot,
  series,
  state,
}: {
  snapshot: MetricsSnapshot | null;
  series: MetricsSeriesResponse | null;
  state: ChartState;
}) {
  const { t } = useLocaleText();
  const router = snapshot?.router;
  const routerPts = series?.router || [];
  return (
    <div className="grid animate-fade-in-up gap-5">
      <KpiGrid>
        <MetricKpiCard
          label={t("metrics.kpi.activeRoutes")}
          value={formatNumber(router?.activeRoutes)}
          detail={t("metrics.detail.pending", { count: formatNumber(router?.pendingRoutes) })}
          icon={<Route />}
          spark={routerPts.map((p) => p.activeRoutes)}
        />
        <MetricKpiCard
          label={t("metrics.kpi.forwardListeners")}
          value={formatNumber(router?.sshForwardListeners)}
          detail={t("metrics.detail.created", { count: formatNumber(router?.sshForwardListenerCreatedTotal) })}
          icon={<Activity />}
          spark={routerPts.map((p) => p.forwardListeners)}
        />
        <MetricKpiCard
          label={t("metrics.kpi.sshSessions")}
          value={formatNumber(router?.sshActiveSessions)}
          detail={t("metrics.detail.activeSessions")}
          icon={<Network />}
          spark={routerPts.map((p) => p.proxyInflight)}
        />
        <MetricKpiCard
          label={t("metrics.kpi.emfile")}
          value={formatNumber(router?.sshForwardEmfileErrorsTotal)}
          detail={t("metrics.detail.tooManyFiles")}
          icon={<ServerCrash />}
          tone={(router?.sshForwardEmfileErrorsTotal || 0) > 0 ? "critical" : "default"}
        />
      </KpiGrid>
      <div className="grid gap-5 xl:grid-cols-2">
        <ResourceTrendChart
          title={t("metrics.chart.routesVsListeners")}
          maxMode="auto"
          state={state}
          series={[
            { label: "Routes", color: "#10B981", values: series?.router.map((p) => p.activeRoutes) || [] },
            { label: "Listeners", color: "#EF4444", values: series?.router.map((p) => p.forwardListeners) || [] },
          ]}
          timestamps={series?.router.map((p) => p.timestamp) || []}
        />
        <ResourceTrendChart
          title={t("metrics.chart.proxyErrorCounters")}
          maxMode="auto"
          state={state}
          hint={t("metrics.chart.deltaHint")}
          series={[
            { label: "Upstream", color: "#EF4444", values: deltaSeries(series?.router.map((p) => p.proxyUpstreamErrorsTotal)) },
            { label: "Health", color: "#F59E0B", values: deltaSeries(series?.router.map((p) => p.healthProbeFailuresTotal)) },
            { label: "DB", color: "#8B5CF6", values: deltaSeries(series?.router.map((p) => p.dbErrorsTotal)) },
          ]}
          timestamps={series?.router.map((p) => p.timestamp) || []}
        />
      </div>
      <CountersTable router={router} />
    </div>
  );
}

function LlmTab({
  snapshot,
  series,
  top,
  failover,
  state,
}: {
  snapshot: MetricsSnapshot | null;
  series: MetricsSeriesResponse | null;
  top: LlmTopResponse | null;
  failover: LlmReliabilityResponse | null;
  state: ChartState;
}) {
  const { t } = useLocaleText();
  const llm = snapshot?.llm;
  const llmPts = series?.llm || [];
  const failoverPct = llm?.failoverSuccessRate != null ? llm.failoverSuccessRate * 100 : null;
  const cachePct = llm?.cacheHitRate != null ? llm.cacheHitRate * 100 : null;
  return (
    <div className="grid animate-fade-in-up gap-5">
      <KpiGrid>
        <MetricKpiCard
          label={t("metrics.kpi.rpm")}
          value={fixed(llm?.rpm)}
          detail={t("metrics.detail.requestsPerMin")}
          icon={<BrainCircuit />}
          emphasize
          spark={llmPts.map((p) => p.rpm)}
        />
        <MetricKpiCard
          label={t("metrics.kpi.tpm")}
          value={compactTokens(llm?.tpm)}
          detail={t("metrics.detail.inOut", {
            in: compactTokens(llm?.inputTpm),
            out: compactTokens(llm?.outputTpm),
          })}
          icon={<Activity />}
          spark={llmPts.map((p) => p.tpm)}
        />
        <MetricKpiCard
          label={t("metrics.kpi.cacheHit")}
          value={cachePct != null ? percent(cachePct) : "-"}
          detail={t("metrics.detail.cacheHit")}
          icon={<Gauge />}
        />
        <MetricKpiCard
          label={t("metrics.kpi.errorRate")}
          value={percent((llm?.errorRate || 0) * 100)}
          detail={t("metrics.detail.rateLimitPerMin", { value: fixed(llm?.rateLimitPerMinute) })}
          icon={<AlertTriangle />}
          tone={(llm?.errorRate || 0) > 0.1 ? "warning" : "default"}
          spark={llmPts.map((p) => p.errorRate * 100)}
        />
        <MetricKpiCard
          label={t("metrics.kpi.failover")}
          value={failoverPct != null ? percent(failoverPct) : "-"}
          detail={t("metrics.detail.substitutionRate", {
            value: percent((failover?.substitutionRate || 0) * 100),
          })}
          icon={<Shuffle />}
          tone={failoverPct != null && failoverPct < 90 ? "warning" : "default"}
        />
      </KpiGrid>
      <div className="grid gap-5 xl:grid-cols-2">
        <ResourceTrendChart
          title={t("metrics.chart.requestErrorTrend")}
          maxMode="auto"
          state={state}
          series={[
            { label: "RPM", color: "#06B6D4", values: series?.llm.map((p) => p.rpm) || [] },
            { label: "429", color: "#F59E0B", values: series?.llm.map((p) => p.rateLimited) || [] },
            { label: "Error %", color: "#EF4444", values: series?.llm.map((p) => p.errorRate * 100) || [] },
          ]}
          timestamps={series?.llm.map((p) => p.timestamp) || []}
        />
        <ResourceTrendChart
          title={t("metrics.chart.tokenTrend")}
          maxMode="auto"
          state={state}
          unit="tok/min"
          series={[
            { label: "TPM", color: "#8B5CF6", values: series?.llm.map((p) => p.tpm) || [] },
            { label: "Input", color: "#6366F1", values: series?.llm.map((p) => p.inputTpm) || [] },
            { label: "Output", color: "#10B981", values: series?.llm.map((p) => p.outputTpm) || [] },
          ]}
          timestamps={series?.llm.map((p) => p.timestamp) || []}
        />
      </div>
      <div className="grid gap-5 xl:grid-cols-[1fr_1fr]">
        <ModelSubstitutionPanel data={failover} />
        <TopConsumersTable top={top} />
      </div>
    </div>
  );
}

function EventsTab({ events }: { events: MetricEvent[] }) {
  return (
    <div className="grid animate-fade-in-up gap-5">
      <MetricEventsList events={events} full />
    </div>
  );
}
