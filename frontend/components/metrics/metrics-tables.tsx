"use client";

import { Card, ScrollShadow } from "@heroui/react";
import * as React from "react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import type { LlmTopResponse, MetricEvent, MetricsSnapshot } from "@/lib/types";
import { compactTokens, formatDateTime, formatNumber, percent } from "@/lib/utils";
import { StatusChip } from "./metrics-cards";

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

export function MetricEventsList({ events, full = false }: { events: MetricEvent[]; full?: boolean }) {
  const { t } = useLocaleText();
  return (
    <Card className="rounded-xl">
      <Card.Header>
        <Card.Title>{t("metrics.panel.recentAlerts")}</Card.Title>
        <Card.Description>{t("metrics.panel.eventsCount", { count: events.length })}</Card.Description>
      </Card.Header>
      <Card.Content>
        <ScrollShadow className={full ? "max-h-[520px]" : "max-h-[320px]"}>
          <div className="grid gap-2 pr-2">
            {events.length === 0 ? (
              <p className="text-sm text-muted-foreground">{t("metrics.panel.noAlerts")}</p>
            ) : (
              events.map((event, index) => (
                <div key={`${event.timestamp}-${event.kind}-${index}`} className="rounded-lg border p-2.5">
                  <div className="flex flex-wrap items-center justify-between gap-2">
                    <div className="flex items-center gap-2">
                      <StatusChip
                        status={
                          event.severity === "critical"
                            ? "critical"
                            : event.severity === "warning"
                              ? "warning"
                              : "healthy"
                        }
                      />
                      <span className="text-sm font-medium">{event.kind}</span>
                    </div>
                    <span className="text-xs text-muted-foreground">
                      {formatDateTime(event.timestamp * 1000)}
                    </span>
                  </div>
                  <p className="mt-2 text-sm">{event.message}</p>
                  {full ? (
                    <pre className="mt-2 overflow-auto rounded bg-muted p-2 text-xs text-muted-foreground">
                      {JSON.stringify(event.details || {}, null, 2)}
                    </pre>
                  ) : null}
                </div>
              ))
            )}
          </div>
        </ScrollShadow>
      </Card.Content>
    </Card>
  );
}
