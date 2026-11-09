"use client";

import * as React from "react";
import { AnimatePresence, MotionConfig, motion } from "motion/react";
import { ArrowRight, Loader2 } from "lucide-react";
import {
  protocolTrafficSummary,
  trafficEnded,
  trafficRuleKind,
  TRAFFIC_BUCKETS,
  TRAFFIC_WINDOW_MS,
  type ShareRouteTraffic,
} from "@/lib/model-routing-traffic";
import type { ModelRouteAttempt, ModelRouteTrafficEvent, ModelRoutingApp, UserModelRoutingShare } from "@/lib/types";
import type { ModelRoutingTrafficStatus } from "@/lib/use-model-routing-traffic";
import type { AppLocale, MessageKey } from "@/lib/i18n";
import { cn, compactTokens } from "@/lib/utils";
import { formatUsdMicros } from "@/lib/usd-micros";

type T = (key: MessageKey, params?: Record<string, string | number>) => string;
const VISIBLE_ROWS = 5;
const EXPANDED_ROWS = 50;
const WINDOW_MINUTES = TRAFFIC_WINDOW_MS / 60_000;
const EASE_OUT = [0.16, 1, 0.3, 1] as const;

const is2xx = (status?: number) => !!status && status >= 200 && status < 300;
const seconds = (ms?: number) => ((ms || 0) / 1000).toFixed(1);

/** Ticks only while something on screen is still in progress. */
function useNow(active: boolean) {
  const [now, setNow] = React.useState(() => Date.now());
  React.useEffect(() => {
    if (!active) return;
    setNow(Date.now());
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [active]);
  return now;
}

function StatusPill({ status, tone }: { status?: number; tone?: "ok" | "bad" | "warn" }) {
  const resolved = tone || (is2xx(status) ? "ok" : "bad");
  return <span className={cn("inline-flex h-5 items-center rounded-full px-1.5 font-mono text-[11px] font-semibold tabular-nums",
    resolved === "ok" ? "bg-emerald-50 text-emerald-700" : resolved === "warn" ? "bg-amber-50 text-amber-700" : "bg-red-50 text-red-700")}>
    {status ?? "—"}
  </span>;
}

function AttemptChip({ attempt, name, t }: { attempt: ModelRouteAttempt; name: string; t: T }) {
  const forwarded = attempt.outcome === "response";
  const reason = t(`modelHub.attempt.${attempt.outcome}` as MessageKey);
  const title = [name, reason, attempt.statusCode ? `HTTP ${attempt.statusCode}` : "", `${seconds(attempt.durationMs)}s`].filter(Boolean).join(" · ");
  return (
    <span title={title} className={cn("inline-flex min-w-0 items-center gap-1.5 rounded-md border px-1.5 py-0.5 text-[11px]",
      !forwarded ? "border-dashed border-slate-300 text-muted-foreground"
        : is2xx(attempt.statusCode) ? "border-emerald-200 bg-emerald-50/60 text-emerald-800" : "border-red-200 bg-red-50/60 text-red-700")}>
      <span className={cn("min-w-0 max-w-[9rem] truncate font-mono", !forwarded && "line-through decoration-slate-300")}>{name}</span>
      {forwarded ? <span className="font-mono font-semibold tabular-nums">{attempt.statusCode ?? "—"}</span>
        : <span className="whitespace-nowrap">{reason}</span>}
    </span>
  );
}

function TrafficPath({ event, names, t }: { event: ModelRouteTrafficEvent; names: Map<string, string>; t: T }) {
  if (!event.routeId) {
    return <span className="text-xs text-muted-foreground">{event.errorCode === "user_model_route_not_configured" ? t("modelHub.traffic.noRule") : event.errorCode || "—"}</span>;
  }
  if (!event.attempts.length) {
    return <span className="inline-flex items-center gap-1 text-xs text-muted-foreground">
      <Loader2 className="size-3 animate-spin motion-reduce:animate-none" aria-hidden />{t("modelHub.traffic.phase.routing")}
    </span>;
  }
  return (
    <span className="flex min-w-0 flex-wrap items-center gap-1">
      {event.attempts.map((attempt, index) => <React.Fragment key={`${attempt.targetShareId}:${index}`}>
        {index ? <><ArrowRight className="size-3 shrink-0 text-muted-foreground/60" aria-hidden /><span className="sr-only">{t("modelHub.traffic.then")}</span></> : null}
        <AttemptChip attempt={attempt} name={names.get(attempt.targetShareId) || attempt.targetShareId} t={t} />
      </React.Fragment>)}
    </span>
  );
}

function TrafficResult({ event, now, t }: { event: ModelRouteTrafficEvent; now: number; t: T }) {
  const elapsed = Math.max(0, now - Date.parse(event.startedAt));
  if (event.phase === "routing") {
    return <span className="text-[11px] text-muted-foreground tabular-nums">{t("modelHub.traffic.elapsed", { seconds: seconds(elapsed) })}</span>;
  }
  if (event.phase === "streaming") {
    return <span className="inline-flex items-center gap-1.5 text-[11px] text-primary tabular-nums">
      <span className="model-route-dot model-route-dot-live bg-primary" aria-hidden />
      {t("modelHub.traffic.phase.streaming", { seconds: seconds(elapsed) })}
    </span>;
  }
  const timing = event.routeId ? [
    event.headersMs !== undefined ? t("modelHub.traffic.headers", { seconds: seconds(event.headersMs) }) : "",
    event.durationMs !== undefined ? t("modelHub.traffic.total", { seconds: seconds(event.durationMs) }) : "",
  ].filter(Boolean).join(" · ") : "";
  return <span className="inline-flex items-center gap-1.5 text-[11px] text-muted-foreground tabular-nums">
    {event.phase === "interrupted" ? <span className="rounded-full bg-amber-50 px-1.5 py-0.5 font-medium text-amber-700">{t("modelHub.traffic.phase.interrupted")}</span> : null}
    <StatusPill status={event.statusCode} />
    {timing ? <span className="whitespace-nowrap">{timing}</span> : null}
  </span>;
}

/** What the answering Share reported. Zero cache counters stay out of the way. */
function TrafficUsage({ event, locale, t }: { event: ModelRouteTrafficEvent; locale: AppLocale; t: T }) {
  const usage = event.usage;
  if (!usage) {
    return event.targetShareId && event.phase === "complete" && is2xx(event.statusCode)
      ? <span className="text-[11px] text-muted-foreground/80">{t("modelHub.traffic.usage.pending")}</span> : null;
  }
  const exact = (value: number) => value.toLocaleString(locale);
  const items: [string, number][] = [
    [t("modelHub.traffic.usage.input"), usage.inputTokens],
    [t("modelHub.traffic.usage.output"), usage.outputTokens],
    ...(usage.cacheReadTokens ? [[t("modelHub.traffic.usage.cacheRead"), usage.cacheReadTokens] as [string, number]] : []),
    ...(usage.cacheWriteTokens ? [[t("modelHub.traffic.usage.cacheWrite"), usage.cacheWriteTokens] as [string, number]] : []),
    [t("modelHub.traffic.usage.total"), usage.totalTokens],
  ];
  const actualModel = usage.actualModel && usage.actualModel !== event.requestedModel ? usage.actualModel : "";
  const cost = formatUsdMicros(usage.costMicros, locale);
  return (
    <span className="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1 text-[11px]">
      {items.map(([label, value]) => (
        <span key={label} className="whitespace-nowrap" title={`${label} ${exact(value)}`}>
          <span className="text-muted-foreground">{label}</span>{" "}
          <span className="font-mono tabular-nums text-foreground">{compactTokens(value, locale)}</span>
        </span>
      ))}
      <span title={t("modelHub.traffic.usage.costHint")}
        className={cn("inline-flex h-5 items-center rounded-full px-1.5 font-mono font-semibold tabular-nums",
          cost ? "bg-primary/10 text-primary" : "bg-secondary text-muted-foreground")}>
        {cost ?? t("modelHub.traffic.usage.unpriced")}
      </span>
      {usage.estimated ? <span className="rounded-full bg-amber-50 px-1.5 py-0.5 text-amber-700">{t("modelHub.traffic.usage.estimated")}</span> : null}
      {actualModel ? <span className="min-w-0 max-w-[16rem] truncate text-muted-foreground" title={actualModel}>
        {t("modelHub.traffic.usage.actualModel", { model: actualModel })}
      </span> : null}
    </span>
  );
}

/** Live, read-only list of the viewer's own unified-API requests for one protocol. */
export function ModelHubTraffic({ events, app, shares, status, locale, t }: {
  events: ModelRouteTrafficEvent[]; app: ModelRoutingApp; shares: UserModelRoutingShare[];
  status: ModelRoutingTrafficStatus; locale: AppLocale; t: T;
}) {
  const [expanded, setExpanded] = React.useState(false);
  const rows = events.filter(event => event.appType === app);
  const summary = protocolTrafficSummary(events, app);
  const visible = rows.slice(0, expanded ? EXPANDED_ROWS : VISIBLE_ROWS);
  const now = useNow(visible.some(event => !trafficEnded(event)));
  const names = new Map(shares.map(share => [share.shareId, share.subdomain || share.shareName]));
  const kindLabel: Record<ReturnType<typeof trafficRuleKind>, string> = {
    default: t("modelHub.traffic.kind.default"), exact: t("modelHub.traffic.kind.exact"), unmatched: t("modelHub.traffic.kind.unmatched"),
  };

  if (status === "unsupported") return <p className="rounded-lg border border-dashed border-border px-3 py-3 text-xs text-muted-foreground">{t("modelHub.traffic.unsupported")}</p>;
  if (status === "idle") return <p className="flex items-center gap-2 px-1 py-2 text-xs text-muted-foreground"><Loader2 className="size-3.5 animate-spin motion-reduce:animate-none" aria-hidden />{t("common.loading")}</p>;

  const stat = "inline-flex items-center gap-1 rounded-full bg-secondary px-2 py-0.5 tabular-nums";
  return (
    <MotionConfig reducedMotion="user">
      <div className="grid min-w-0 gap-2">
        <div className="flex min-w-0 flex-wrap items-center gap-1.5 text-[11px] text-muted-foreground">
          <span className={stat}>{t("modelHub.traffic.summary", { minutes: WINDOW_MINUTES, total: summary.total })}</span>
          {summary.ended ? <span className={stat}>{t("modelHub.traffic.successRate", { rate: Math.round((summary.succeeded / summary.ended) * 100) })}</span> : null}
          {summary.continued ? <span className={cn(stat, "bg-amber-50 text-amber-700")}>{t("modelHub.traffic.continued", { count: summary.continued })}</span> : null}
          {summary.inflight ? <span className={cn(stat, "bg-primary/10 text-primary")}>{t("modelHub.traffic.inflight", { count: summary.inflight })}</span> : null}
          {summary.tokens ? <span className={stat} title={t("modelHub.traffic.usage.costHint")}>
            {summary.priced
              ? t(summary.unpriced ? "modelHub.traffic.usage.summaryPartial" : "modelHub.traffic.usage.summary", {
                tokens: compactTokens(summary.tokens, locale), cost: formatUsdMicros(summary.costMicros, locale) || "—", unpriced: summary.unpriced,
              })
              : t("modelHub.traffic.usage.summaryTokens", { tokens: compactTokens(summary.tokens, locale) })}
          </span> : null}
          {status === "error" ? <span role="status" className="text-amber-700">{t("modelHub.traffic.error")}</span> : null}
        </div>
        {rows.length ? (
          <ol className="grid min-w-0 divide-y divide-border overflow-hidden rounded-xl border border-border bg-card" aria-label={t("modelHub.traffic.title")}>
            <AnimatePresence initial={false}>
              {visible.map(event => (
                <motion.li key={event.sequence} layout="position"
                  initial={{ opacity: 0, y: -6 }} animate={{ opacity: 1, y: 0 }} exit={{ opacity: 0 }}
                  transition={{ duration: 0.35, ease: EASE_OUT }}
                  className={cn("grid min-w-0 items-center gap-x-3 gap-y-1.5 px-3 py-2 sm:grid-cols-[4.25rem_minmax(0,11rem)_minmax(0,1fr)_auto]",
                    event.phase === "interrupted" && "bg-amber-50/30", !trafficEnded(event) && "bg-primary/[0.02]")}>
                  <time dateTime={event.startedAt} className="font-mono text-[11px] tabular-nums text-muted-foreground">
                    {new Date(event.startedAt).toLocaleTimeString(locale, { hour12: false })}
                  </time>
                  <span className="flex min-w-0 items-center gap-1.5">
                    <span className="min-w-0 truncate font-mono text-xs text-foreground" title={event.requestedModel}>{event.requestedModel}</span>
                    <span className={cn("shrink-0 rounded-full px-1.5 py-px text-[10px] font-medium",
                      event.routeId ? "bg-primary/10 text-primary" : "bg-slate-100 text-slate-600")}>{kindLabel[trafficRuleKind(event)]}</span>
                  </span>
                  <TrafficPath event={event} names={names} t={t} />
                  <span className="sm:justify-self-end"><TrafficResult event={event} now={now} t={t} /></span>
                  <span className="min-w-0 empty:hidden sm:col-span-3 sm:col-start-2"><TrafficUsage event={event} locale={locale} t={t} /></span>
                </motion.li>
              ))}
            </AnimatePresence>
          </ol>
        ) : <p className="rounded-xl border border-dashed border-border px-4 py-5 text-center text-xs text-muted-foreground">{t("modelHub.traffic.empty", { minutes: WINDOW_MINUTES })}</p>}
        {rows.length > VISIBLE_ROWS ? (
          <button type="button" onClick={() => setExpanded(value => !value)}
            className="min-h-11 w-fit rounded-lg px-2 text-xs font-medium text-primary hover:bg-primary/5 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring">
            {expanded ? t("modelHub.traffic.showLess") : t("modelHub.traffic.showAll", { count: Math.min(rows.length, EXPANDED_ROWS) })}
          </button>
        ) : null}
      </div>
    </MotionConfig>
  );
}

/** Queue-row counters: what this Share actually did for this rule in the window. */
export function QueueRowTraffic({ traffic, t }: { traffic?: ShareRouteTraffic; t: T }) {
  if (!traffic || (!traffic.forwarded && !traffic.landedAfterFailure)) {
    return <span className="block pl-6 text-[11px] text-muted-foreground/70">{t("modelHub.traffic.queue.idle", { minutes: WINDOW_MINUTES })}</span>;
  }
  const peak = Math.max(1, ...traffic.buckets);
  return (
    <span className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-0.5 pl-6 text-[11px] text-muted-foreground">
      <svg width={TRAFFIC_BUCKETS * 3} height={14} viewBox={`0 0 ${TRAFFIC_BUCKETS * 3} 14`} className="shrink-0" aria-hidden>
        {traffic.buckets.map((count, index) => {
          const height = count ? Math.max(2, Math.round((count / peak) * 14)) : 1;
          return <rect key={index} x={index * 3} y={14 - height} width={2} height={height} rx={0.5}
            className={count ? "fill-primary/70" : "fill-slate-200"} />;
        })}
      </svg>
      <span className="sr-only">{t("modelHub.traffic.queue.chart", { minutes: WINDOW_MINUTES })}</span>
      <span className="tabular-nums">{t("modelHub.traffic.queue.forwarded", { count: traffic.forwarded })}</span>
      {traffic.failed ? <span className="tabular-nums text-red-600">{t("modelHub.traffic.queue.failed", { count: traffic.failed })}</span> : null}
      {traffic.landedAfterFailure ? <span className="tabular-nums text-amber-700">{t("modelHub.traffic.queue.landed", { count: traffic.landedAfterFailure })}</span> : null}
    </span>
  );
}
