"use client";

import { Card, ScrollShadow } from "@heroui/react";
import * as React from "react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import type { LlmReliabilityResponse, LlmTopResponse, MetricEvent, MetricsSnapshot } from "@/lib/types";
import { compactTokens, fixed, formatNumber, formatDateTime, percent } from "@/lib/utils";

export function SimpleTable({ headers, rows }: { headers: string[]; rows: React.ReactNode[][] }) {
  const { t } = useLocaleText();
  return (
    <div className="overflow-x-auto">
      <table className="w-full text-left text-sm">
        <thead className="text-xs text-muted-foreground">
          <tr>
            {headers.map((h) => (
              <th key={h} className="border-b px-2.5 py-1.5 font-medium">
                {h}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {rows.length === 0 ? (
            <tr>
              <td className="px-2.5 py-3 text-muted-foreground" colSpan={headers.length}>
                {t("metrics.table.noData")}
              </td>
            </tr>
          ) : (
            rows.map((row, i) => (
              <tr key={i}>
                {row.map((cell, j) => (
                  <td key={j} className="border-b px-2.5 py-1.5">
                    {cell}
                  </td>
                ))}
              </tr>
            ))
          )}
        </tbody>
      </table>
    </div>
  );
}

export function CountersTable({ router }: { router?: MetricsSnapshot["router"] }) {
  const { t } = useLocaleText();
  const rows: Array<[string, number | undefined]> = [
    [t("metrics.counter.listenerCreated"), router?.sshForwardListenerCreatedTotal],
    [t("metrics.counter.listenerShutdown"), router?.sshForwardListenerShutdownTotal],
    [t("metrics.counter.bindErrors"), router?.sshForwardBindErrorsTotal],
    [t("metrics.counter.acceptErrors"), router?.sshForwardAcceptErrorsTotal],
    [t("metrics.counter.emfileErrors"), router?.sshForwardEmfileErrorsTotal],
    [t("metrics.counter.pendingChannelOpens"), router?.sshPendingChannelOpens],
    [t("metrics.counter.channelOpenStarted"), router?.sshChannelOpenStartedTotal],
    [t("metrics.counter.channelOpenSucceeded"), router?.sshChannelOpenSucceededTotal],
    [t("metrics.counter.channelOpenExplicitFailures"), router?.sshChannelOpenExplicitFailuresTotal],
    [t("metrics.counter.channelOpenTimeouts"), router?.sshChannelOpenTimeoutTotal],
    [t("metrics.counter.channelOpenSessionErrors"), router?.sshChannelOpenSessionErrorsTotal],
    [t("metrics.counter.channelOpenCancelled"), router?.sshChannelOpenCancelledTotal],
    [t("metrics.counter.activeBridges"), router?.sshActiveBridges],
    [t("metrics.counter.bridgeCreated"), router?.sshBridgeCreatedTotal],
    [t("metrics.counter.bridgeCompleted"), router?.sshBridgeCompletedTotal],
    [t("metrics.counter.bridgeCancelled"), router?.sshBridgeCancelledTotal],
    [t("metrics.counter.bridgeWriteStalls"), router?.sshBridgeWriteStallTotal],
    [t("metrics.counter.bridgeHalfCloseIdle"), router?.sshBridgeHalfCloseIdleTotal],
    [t("metrics.counter.bridgeIoErrors"), router?.sshBridgeIoErrorsTotal],
    [t("metrics.counter.forwardCapacityRejected"), router?.sshForwardCapacityRejectedTotal],
    [t("metrics.counter.shareActiveRequests"), router?.shareActiveRequests],
    [t("metrics.counter.shareOldestInflightAge"), router?.shareOldestInflightAgeSecs],
    [t("metrics.counter.shareOldestProgressAge"), router?.shareOldestProgressAgeSecs],
    [t("metrics.counter.shareWatchdogReleases"), router?.shareRequestWatchdogForcedReleaseTotal],
    [t("metrics.counter.shareManualReleases"), router?.shareRequestManualReleaseTotal],
    [t("metrics.counter.requestBodyTimeouts"), router?.proxyRequestBodyTimeoutTotal],
    [t("metrics.counter.responseHeaderTimeouts"), router?.proxyResponseHeaderTimeoutTotal],
    [t("metrics.counter.downstreamStallTimeouts"), router?.proxyDownstreamStallTimeoutTotal],
    [t("metrics.counter.requestHardTimeouts"), router?.proxyRequestHardTimeoutTotal],
    [t("metrics.counter.streamSemanticTerminals"), router?.proxyStreamSemanticTerminalTotal],
    [t("metrics.counter.streamFirstEventTimeouts"), router?.proxyStreamFirstEventTimeoutTotal],
    [t("metrics.counter.streamIdleTimeouts"), router?.proxyStreamIdleTimeoutTotal],
    [t("metrics.counter.streamParserOverflows"), router?.proxyStreamParserOverflowTotal],
    [t("metrics.counter.streamUpstreamErrors"), router?.proxyStreamUpstreamErrorsTotal],
    [t("metrics.counter.healthCachedFailures"), router?.healthProbeCachedFailuresTotal],
    [t("metrics.counter.dbErrors"), router?.dbErrorsTotal],
  ];
  return (
    <Card className="rounded-xl">
      <Card.Header>
        <Card.Title>{t("metrics.panel.counters")}</Card.Title>
      </Card.Header>
      <Card.Content>
        <SimpleTable
          headers={[t("metrics.header.metric"), t("metrics.header.value")]}
          rows={rows.map(([a, b]) => [a, formatNumber(b)])}
        />
      </Card.Content>
    </Card>
  );
}

export function TopConsumersTable({ top }: { top: LlmTopResponse | null }) {
  const { t } = useLocaleText();
  return (
    <Card className="rounded-xl">
      <Card.Header>
        <Card.Title>{t("metrics.panel.topConsumers")}</Card.Title>
        <Card.Description>{t("metrics.panel.groupedBy", { by: top?.by || "tokens" })}</Card.Description>
      </Card.Header>
      <Card.Content>
        <SimpleTable
          headers={[
            t("metrics.header.key"),
            t("metrics.header.requests"),
            t("metrics.header.tokens"),
            t("metrics.header.errors"),
            t("metrics.header.errorRate"),
            t("metrics.header.p95"),
          ]}
          rows={(top?.items || []).map((item) => [
            item.key,
            formatNumber(item.requests),
            compactTokens(item.totalTokens),
            formatNumber(item.errors),
            percent(item.errorRate * 100),
            item.p95LatencyMs ? `${item.p95LatencyMs}ms` : "-",
          ])}
        />
      </Card.Content>
    </Card>
  );
}

export function SharePerformanceTable({ data }: { data: LlmTopResponse | null }) {
  const { locale, t } = useLocaleText();
  const rows = [...(data?.items || [])].sort((left, right) => {
    const leftHasTtft = left.averageTtftMs != null && Number.isFinite(left.averageTtftMs);
    const rightHasTtft = right.averageTtftMs != null && Number.isFinite(right.averageTtftMs);
    if (leftHasTtft !== rightHasTtft) {
      return rightHasTtft ? 1 : -1;
    }
    return Number(right.averageTtftMs || 0) - Number(left.averageTtftMs || 0);
  });
  return (
    <Card className="rounded-xl">
      <Card.Header>
        <Card.Title>{t("metrics.panel.sharePerformance")}</Card.Title>
        <Card.Description>{t("metrics.panel.sharePerformanceDesc")}</Card.Description>
      </Card.Header>
      <Card.Content>
        <SimpleTable
          headers={[
            t("metrics.header.share"),
            t("metrics.header.requests"),
            "TTFT",
            "TPS",
            t("metrics.header.samples"),
            t("metrics.header.lastRequest"),
          ]}
          rows={rows.map((item) => [
            <span key="share" className="font-mono text-xs">{item.key}</span>,
            formatNumber(item.requests),
            item.averageTtftMs != null ? `${fixed(item.averageTtftMs)} ms` : "-",
            item.averageTps != null ? `${fixed(item.averageTps)} tok/s` : "-",
            `${formatNumber(item.ttftSampleCount)} / ${formatNumber(item.tpsSampleCount)}`,
            item.lastRequestAt ? formatDateTime(item.lastRequestAt * 1000, locale) : "-",
          ])}
        />
      </Card.Content>
    </Card>
  );
}

export function ModelSubstitutionPanel({ data }: { data: LlmReliabilityResponse | null }) {
  const { t } = useLocaleText();
  const subRate = data ? data.substitutionRate * 100 : 0;
  const successRate =
    data?.substitutionSuccessRate != null ? data.substitutionSuccessRate * 100 : null;
  return (
    <Card className="rounded-2xl">
      <Card.Header>
        <Card.Title className="text-base font-semibold">
          {t("metrics.panel.substitution")}
        </Card.Title>
        <Card.Description>{t("metrics.panel.substitutionDesc")}</Card.Description>
      </Card.Header>
      <Card.Content>
        <div className="mb-4 grid grid-cols-3 gap-3">
          <div className="rounded-xl border bg-muted/30 p-3">
            <div className="text-xs text-muted-foreground">{t("metrics.sub.total")}</div>
            <div className="mt-1 font-display text-xl">{formatNumber(data?.totalRequests)}</div>
          </div>
          <div className="rounded-xl border bg-muted/30 p-3">
            <div className="text-xs text-muted-foreground">{t("metrics.sub.rate")}</div>
            <div className="mt-1 font-display text-xl">{percent(subRate)}</div>
            <div className="text-[11px] text-muted-foreground">
              {formatNumber(data?.substitutedRequests)}
            </div>
          </div>
          <div className="rounded-xl border bg-muted/30 p-3">
            <div className="text-xs text-muted-foreground">{t("metrics.sub.success")}</div>
            <div className="mt-1 font-display text-xl">
              {successRate != null ? percent(successRate) : "-"}
            </div>
          </div>
        </div>
        <SimpleTable
          headers={[
            t("metrics.header.requested"),
            t("metrics.header.actual"),
            t("metrics.header.requests"),
            t("metrics.header.errorRate"),
          ]}
          rows={(data?.items || []).map((item) => [
            <span key="r" className="font-mono text-xs">{item.requestedModel}</span>,
            <span key="a" className="font-mono text-xs text-accent">{item.actualModel}</span>,
            formatNumber(item.requests),
            percent(item.errorRate * 100),
          ])}
        />
      </Card.Content>
    </Card>
  );
}

export function MetricEventsList({ events, full = false }: { events: MetricEvent[]; full?: boolean }) {
  const { t } = useLocaleText();
  const dotFor = (severity: string) =>
    severity === "critical" ? "bg-red-500" : severity === "warning" ? "bg-amber-500" : "bg-slate-400";
  return (
    <Card className="rounded-2xl">
      <Card.Header>
        <Card.Title className="text-base font-semibold">
          {t("metrics.panel.recentAlerts")}
        </Card.Title>
        <Card.Description>{t("metrics.panel.eventsCount", { count: events.length })}</Card.Description>
      </Card.Header>
      <Card.Content>
        <ScrollShadow className={full ? "max-h-[560px]" : "max-h-[320px]"}>
          {events.length === 0 ? (
            <p className="text-sm text-muted-foreground">{t("metrics.panel.noAlerts")}</p>
          ) : (
            <ol className="relative grid gap-1 pr-2">
              <span className="absolute bottom-2 left-[5px] top-2 w-px bg-border" aria-hidden />
              {events.map((event, index) => (
                <li
                  key={`${event.timestamp}-${event.kind}-${index}`}
                  className="relative grid grid-cols-[16px_1fr] gap-3 rounded-lg py-2 pl-0.5 pr-2 transition-colors hover:bg-accent/[0.04]"
                >
                  <span className={`relative z-10 mt-1.5 h-2.5 w-2.5 rounded-full ring-4 ring-card ${dotFor(event.severity)}`} />
                  <div className="min-w-0">
                    <div className="flex flex-wrap items-center justify-between gap-2">
                      <span className="font-mono text-xs uppercase text-foreground/80">
                        {event.kind}
                      </span>
                      <span className="text-xs text-muted-foreground">
                        {formatDateTime(event.timestamp * 1000)}
                      </span>
                    </div>
                    <p className="mt-1 text-sm">{event.message}</p>
                    {full ? (
                      <pre className="mt-2 overflow-auto rounded-lg bg-muted p-2 text-xs text-muted-foreground">
                        {JSON.stringify(event.details || {}, null, 2)}
                      </pre>
                    ) : null}
                  </div>
                </li>
              ))}
            </ol>
          )}
        </ScrollShadow>
      </Card.Content>
    </Card>
  );
}
