"use client";

import * as React from "react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { useDashboardFocus } from "@/components/dashboard/dashboard-focus";
import type { DashboardResponse, MapPoint, MarketRequestLog, RecentRequestEvent, ShareRequestLog } from "@/lib/types";
import { cn } from "@/lib/utils";
import { useMapDisplaySettings, computeMapOffsetY } from "@/lib/map-display-settings";

function projectPoint(point: MapPoint) {
  if (typeof point.lat !== "number" || typeof point.lon !== "number") return null;
  const x = ((point.lon + 180) / 360) * 100;
  const y = ((90 - point.lat) / 180) * 100;
  const xPct = Math.max(1, Math.min(99, x));
  const yPct = Math.max(1, Math.min(99, y));
  return { x: xPct * 3.6, y: yPct * 1.8, xPct, yPct };
}

type PlacedPoint = NonNullable<ReturnType<typeof projectPoint>>;
type TickerMeta = Partial<Omit<ShareRequestLog, "createdAt"> & Omit<MarketRequestLog, "createdAt">> & {
  createdAt?: string | number;
  shareName?: string;
};

const REQUEST_TICKER_LIMIT = 6;
const MAP_VIEWPORT_HEIGHT_PX = 420;

function spreadPoints(points: PlacedPoint[], minDistPct: number, lockedIndex: number) {
  if (points.length < 2) return points;
  const placed = points.map((point) => ({ ...point }));
  for (let iteration = 0; iteration < 28; iteration++) {
    let moved = false;
    for (let i = 0; i < placed.length; i++) {
      for (let j = i + 1; j < placed.length; j++) {
        const a = placed[i];
        const b = placed[j];
        let dx = b.xPct - a.xPct;
        let dy = b.yPct - a.yPct;
        let d = Math.hypot(dx, dy);
        if (d < 0.0001) {
          const angle = ((i * 137.5 + j * 23.4) % 360) * (Math.PI / 180);
          dx = Math.cos(angle);
          dy = Math.sin(angle);
          d = 1;
        }
        if (d >= minDistPct) continue;
        const overlap = minDistPct - d;
        const ux = dx / d;
        const uy = dy / d;
        const aLocked = i === lockedIndex;
        const bLocked = j === lockedIndex;
        if (aLocked && bLocked) continue;
        if (aLocked) {
          b.xPct += ux * overlap;
          b.yPct += uy * overlap;
        } else if (bLocked) {
          a.xPct -= ux * overlap;
          a.yPct -= uy * overlap;
        } else {
          a.xPct -= (ux * overlap) / 2;
          a.yPct -= (uy * overlap) / 2;
          b.xPct += (ux * overlap) / 2;
          b.yPct += (uy * overlap) / 2;
        }
        moved = true;
      }
    }
    if (!moved) break;
  }
  return placed.map((point) => {
    const xPct = Math.max(1, Math.min(99, point.xPct));
    const yPct = Math.max(1, Math.min(99, point.yPct));
    return { x: xPct * 3.6, y: yPct * 1.8, xPct, yPct };
  });
}

function countryFlag(code?: string) {
  const cc = (code || "").trim().slice(0, 2).toUpperCase();
  if (!/^[A-Z]{2}$/.test(cc)) return "·";
  return String.fromCodePoint(...[...cc].map((ch) => 127397 + ch.charCodeAt(0)));
}

function formatTickerTime(value?: string | number, fallbackSeconds?: string | number) {
  let timestamp = typeof value === "number" ? value : Date.parse(value || "");
  const fallback = Number(fallbackSeconds || 0);
  if (!Number.isFinite(timestamp) && Number.isFinite(fallback) && fallback > 0) {
    timestamp = fallback * 1000;
  }
  const date = Number.isFinite(timestamp) ? new Date(timestamp) : new Date();
  if (!Number.isFinite(date.getTime())) return "--:--:--";
  return new Intl.DateTimeFormat(undefined, {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
    hour12: false,
  }).format(date);
}

function tokenCount(value?: string | number | null) {
  const count = Number(value || 0);
  return Number.isFinite(count) && count > 0 ? count : 0;
}

function usageBucketTotalTokens(log?: Pick<TickerMeta, "inputTokens" | "outputTokens" | "cacheReadTokens" | "cacheCreationTokens"> | null) {
  return tokenCount(log?.inputTokens) + tokenCount(log?.outputTokens) + tokenCount(log?.cacheReadTokens) + tokenCount(log?.cacheCreationTokens);
}

function compactTickerTokens(value: number) {
  const count = Math.max(0, Number(value || 0));
  if (!Number.isFinite(count)) return "0";
  if (count < 1000) return String(Math.round(count));
  const unit = count >= 1_000_000 ? { suffix: "m", value: 1_000_000 } : { suffix: "k", value: 1000 };
  const compact = count / unit.value;
  const text = compact >= 10 ? compact.toFixed(0) : compact.toFixed(1);
  return `${text.replace(/\.0$/, "")}${unit.suffix}`;
}

function formatTickerLatency(value?: number) {
  const ms = Number(value || 0);
  if (!Number.isFinite(ms) || ms < 0) return "-";
  if (ms < 1000) return `${Math.round(ms)}ms`;
  const seconds = ms / 1000;
  const text = seconds >= 10 ? seconds.toFixed(1) : seconds.toFixed(2);
  return `${text.replace(/\.0+$/, "").replace(/(\.\d*[1-9])0+$/, "$1")}s`;
}

function formatMarketFee(value?: string | number) {
  if (value == null || value === "") return "";
  const amount = Number(value);
  if (!Number.isFinite(amount)) return "";
  if (amount > 0 && amount < 0.0001) return `$${amount.toFixed(8)}`;
  if (amount > 0 && amount < 0.01) return `$${amount.toFixed(6)}`;
  return `$${amount.toFixed(amount >= 1 ? 2 : 4)}`;
}

function tickerDetail(meta?: TickerMeta) {
  if (meta?.isHealthCheck) {
    const model = meta.requestedModel || meta.requestModel || meta.actualModel || meta.model || "-";
    const status = meta.statusCode ?? meta.status ?? "-";
    return [meta.requestAgent || meta.appType || "", model, String(status), formatTickerLatency(meta.latencyMs)].filter(Boolean).join(" · ");
  }
  const agent = meta?.requestAgent || "";
  const requested = meta?.requestedModel || meta?.requestModel || "";
  const actual = meta?.actualModel || meta?.model || "";
  const modelName = [agent, requested && actual && requested !== actual ? `${requested} -> ${actual}` : actual || requested || "-"].filter(Boolean).join(" · ");
  const status = meta?.statusCode ?? meta?.status ?? "-";
  const latency = formatTickerLatency(meta?.latencyMs);
  const tokenTotal = usageBucketTotalTokens(meta);
  const tokens = `${compactTickerTokens(tokenTotal)} token${tokenTotal === 1 ? "" : "s"}`;
  const fee = formatMarketFee(meta?.usageAmountUsd);
  const parts = [
    meta?.userEmail || "",
    modelName,
    String(status),
    latency,
    tokens,
    fee,
  ].filter(Boolean);
  return parts.join(" · ");
}

function resolveMapHeatCounts(data: DashboardResponse | null, showHeat: boolean) {
  if (!showHeat || !data) return {};
  // Client installation density is stable across dashboard polls. Request ticker
  // country counts fluctuate every few seconds and made shared country fills flash.
  return data.countryCounts || {};
}

function buildRequestMeta(data: DashboardResponse | null) {
  const marketMeta = new Map<string, MarketRequestLog>();
  const meta = new Map<string, TickerMeta>();
  for (const log of data?.marketRequestLogs || []) {
    marketMeta.set(log.requestId, log);
  }
  for (const share of data?.tickerShares || []) {
    for (const log of share.recentRequests || []) {
      const market = marketMeta.get(log.requestId);
      meta.set(log.requestId, { ...log, shareName: share.shareName, shareId: share.shareId, userEmail: log.userEmail || market?.userEmail, apiKeyPrefix: market?.apiKeyPrefix, usageAmountUsd: market?.usageAmountUsd });
    }
  }
  // P7 Step 2：share.recentRequests 现在直接来自顶层 data.shares 数组，
  // 不再走 clients[*].share。tickerShares 是 router 高频推送的"最近活跃 share"
  // 子集，先于完整 shares 列表填一遍，再用 shares 兜底补全。
  for (const share of data?.shares || []) {
    for (const log of share.recentRequests || []) {
      const market = marketMeta.get(log.requestId);
      meta.set(log.requestId, { ...log, shareName: share.shareName || log.shareName, shareId: share.shareId || log.shareId, userEmail: log.userEmail || market?.userEmail, apiKeyPrefix: market?.apiKeyPrefix, usageAmountUsd: market?.usageAmountUsd });
    }
  }
  for (const [requestId, log] of marketMeta) {
    const existing = meta.get(requestId);
    meta.set(requestId, { ...(existing || {}), ...log, userEmail: log.userEmail || existing?.userEmail });
  }
  return meta;
}

function RequestTicker({ data }: { data: DashboardResponse | null }) {
  const focus = useDashboardFocus();
  const meta = React.useMemo(() => buildRequestMeta(data), [data]);
  const events = React.useMemo(() => {
    return [...(data?.recentRequestEvents || [])]
      .sort((a, b) => new Date(b.startedAt || b.createdAt || 0).getTime() - new Date(a.startedAt || a.createdAt || 0).getTime())
      .slice(0, REQUEST_TICKER_LIMIT)
      .reverse();
  }, [data]);

  if (!events.length) return null;

  return (
    <div className="activity-feed-mask pointer-events-none absolute bottom-[52px] left-3 z-30 flex w-[min(46%,520px)] flex-col gap-1">
      {events.map((event, index) => {
        const item = meta.get(event.requestId);
        const eventUserEmail = event.userEmail;
        const mergedItem = event.isHealthCheck
          ? {
              ...(item || {}),
              userEmail: item?.userEmail || eventUserEmail,
              isHealthCheck: true,
              requestAgent: event.healthAppType || item?.requestAgent || "",
              requestedModel: event.healthModel || item?.requestedModel || item?.requestModel || "",
              status: event.healthStatus || item?.status,
            }
          : item
            ? { ...item, userEmail: item.userEmail || eventUserEmail }
            : eventUserEmail
              ? { userEmail: eventUserEmail }
              : undefined;
        const country = event.userCountry || event.countryCode || "--";
        const subdomain = event.shareSubdomain || event.subdomain || event.shareName || mergedItem?.shareName || "share";
        const eventKey = [event.requestId, event.startedAt || event.createdAt || ""].join(":");
        const statusCode = Number(mergedItem?.statusCode || 0);
        const rawStatus = String(mergedItem?.status || event.healthStatus || "").toLowerCase();
        const failed = statusCode >= 400 || ["failed", "error", "offline"].includes(rawStatus);
        const badge = event.isHealthCheck ? "HC" : statusCode ? String(statusCode) : rawStatus ? rawStatus.slice(0, 3).toUpperCase() : "—";
        return (
          <button type="button" data-map-control key={eventKey} onClick={() => focus.setFocus({ kind: "request", id: event.requestId, source: "activity" })} className={`pointer-events-auto flex max-w-full items-center gap-2 overflow-hidden rounded-lg border px-2.5 py-1.5 text-left text-[10px] text-slate-700 backdrop-blur-md transition-colors ${index === events.length - 1 ? "activity-feed-enter" : ""} ${focus.isFocused("request", event.requestId) ? "border-primary bg-white ring-2 ring-primary/20" : "border-slate-200/80 bg-white/75 hover:bg-white"}`}>
            <span className="font-mono text-slate-500">{formatTickerTime(event.startedAt || event.createdAt, item?.createdAt)}</span>
            <span className={`inline-flex h-[15px] shrink-0 items-center rounded px-1.5 font-mono text-[9px] font-semibold ${event.isHealthCheck ? "bg-blue-100 text-blue-700" : failed ? "bg-rose-100 text-rose-700" : "bg-emerald-100 text-emerald-700"}`}>{badge}</span>
            <span className="min-w-0 truncate text-[11px] text-slate-700"><strong className="font-semibold">{subdomain}</strong> · {countryFlag(country)} {country} · {tickerDetail(mergedItem)}</span>
          </button>
        );
      })}
    </div>
  );
}

export function LiveMap({ data }: { data: DashboardResponse | null }) {
  const { t } = useLocaleText();
  const focus = useDashboardFocus();
  const shellRef = React.useRef<HTMLDivElement | null>(null);
  const worldRef = React.useRef<HTMLDivElement | null>(null);
  const [worldSvg, setWorldSvg] = React.useState("");
  const [mapOffsetY, setMapOffsetY] = React.useState(0);
  const { showFlows, showHeat, viewport } = useMapDisplaySettings();
  const clients = data?.map?.clients || [];
  const server = data?.map?.server;
  const points = [server, ...clients].filter(Boolean) as MapPoint[];
  const placed = React.useMemo(() => {
    const raw = [
      ...(server ? [server] : []),
      ...[...clients].sort((a, b) => (a.id || "").localeCompare(b.id || "")),
    ];
    const projected = raw.map((point) => ({ point, pos: projectPoint(point) })).filter((item): item is { point: MapPoint; pos: PlacedPoint } => !!item.pos);
    const positions = spreadPoints(projected.map((item) => item.pos), 2.6, server ? 0 : -1);
    return projected.map((item, index) => ({ ...item, pos: positions[index] }));
  }, [clients, server]);
  const serverPlaced = placed.find((item) => item.point.pointType === "server");
  const clientPlaced = placed.filter((item) => item.point.pointType !== "server");
  const requestFlows = React.useMemo(() => {
    const shareToClient = new Map<string, string>();
    for (const client of data?.clients || []) {
      for (const shareId of client.shareIds || []) shareToClient.set(shareId, client.installation.id);
    }
    const meta = buildRequestMeta(data);
    const flows = new Map<string, { inflight: number; failures: number; highLatency: number }>();
    for (const event of (data?.recentRequestEvents || []).slice(-200)) {
      if (!event.isInflight) continue;
      const clientId = event.shareId ? shareToClient.get(event.shareId) : undefined;
      if (!clientId) continue;
      const item = meta.get(event.requestId);
      const statusCode = Number(item?.statusCode || 0);
      const status = String(item?.status || event.healthStatus || "").toLowerCase();
      const latency = Number(event.latencyMs || item?.latencyMs || 0);
      const flow = flows.get(clientId) || { inflight: 0, failures: 0, highLatency: 0 };
      flow.inflight += 1;
      if (statusCode >= 400 || ["failed", "error", "offline"].includes(status)) flow.failures += 1;
      if (latency >= 2000) flow.highLatency += 1;
      flows.set(clientId, flow);
    }
    return flows;
  }, [data]);

  React.useLayoutEffect(() => {
    const shell = shellRef.current;
    if (!shell) return;
    const updateOffset = () => {
      const height = shell.clientHeight || MAP_VIEWPORT_HEIGHT_PX;
      setMapOffsetY(computeMapOffsetY(viewport, shell.clientWidth, height));
    };
    updateOffset();
    const observer = new ResizeObserver(updateOffset);
    observer.observe(shell);
    return () => observer.disconnect();
  }, [viewport]);

  React.useEffect(() => {
    let cancelled = false;
    fetch("/world-map.svg", { cache: "force-cache" })
      .then((response) => response.text())
      .then((svg) => {
        if (!cancelled) setWorldSvg(svg);
      })
      .catch(() => {
        if (!cancelled) setWorldSvg("");
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const heatCountsKey = React.useMemo(
    () => JSON.stringify(resolveMapHeatCounts(data, showHeat)),
    [data?.countryCounts, showHeat],
  );

  React.useEffect(() => {
    const root = worldRef.current;
    if (!root) return;
    const counts = JSON.parse(heatCountsKey) as Record<string, number>;
    const values = Object.values(counts).filter((value) => value > 0);
    const max = values.length ? Math.max(...values) : 0;
    for (const element of Array.from(root.querySelectorAll<SVGElement>(".country"))) {
      const iso3 = Array.from(element.classList).find((name) => /^[A-Z]{3}$/.test(name));
      const count = iso3 ? counts[iso3] || 0 : 0;
      const heat = max > 0 ? Math.min(1, count / max) : 0;
      const fillOpacity = String(0.1 + heat * 0.55);
      const strokeOpacity = String(0.16 + heat * 0.4);
      if (!element.style.transition) {
        element.style.transition = "fill-opacity 0.8s ease-out, stroke-opacity 0.8s ease-out";
      }
      if (element.style.fillOpacity !== fillOpacity) element.style.fillOpacity = fillOpacity;
      if (element.style.strokeOpacity !== strokeOpacity) element.style.strokeOpacity = strokeOpacity;
    }
  }, [heatCountsKey, worldSvg]);

  return (
    <section
      ref={shellRef}
      className="relative h-[420px] overflow-hidden rounded-[20px] border bg-white text-primary shadow-[0_4px_6px_rgba(15,23,42,0.04),0_12px_28px_rgba(15,23,42,0.05)]"
      aria-label={t("map.aria")}
    >
      <div className="pointer-events-none absolute inset-0 z-10 bg-[radial-gradient(circle,rgba(15,23,42,0.05)_1px,transparent_1px)] bg-[length:28px_28px] bg-[position:14px_14px]" />
      <div className="pointer-events-none absolute inset-0 z-10 bg-[radial-gradient(circle_at_6%_12%,rgba(0,82,255,0.10),transparent_38%),radial-gradient(circle_at_94%_88%,rgba(77,124,255,0.07),transparent_42%)]" />
      <RequestTicker data={data} />
      <div
        className="absolute left-1/2 top-1/2 z-20 aspect-[2/1] w-full origin-center"
        style={{ transform: `translate(-50%, -50%) translate(0px, ${mapOffsetY}px)` }}
      >
        {worldSvg ? (
          <div
            ref={worldRef}
            className="pointer-events-none absolute inset-0 text-primary [&_svg]:absolute [&_svg]:inset-0 [&_svg]:block [&_svg]:h-full [&_svg]:w-full"
            aria-hidden="true"
            dangerouslySetInnerHTML={{ __html: worldSvg }}
          />
        ) : (
          <div className="pointer-events-none absolute inset-0 bg-[url('/world-map.svg')] bg-[length:100%_100%] bg-center bg-no-repeat" aria-hidden="true" />
        )}
        <svg className="absolute inset-0 h-full w-full overflow-visible" viewBox="0 0 360 180" preserveAspectRatio="none" aria-hidden="true">
          {showFlows && serverPlaced
            ? clientPlaced.map(({ point: client, pos: b }) => {
                const a = serverPlaced.pos;
                const flow = requestFlows.get(client.id);
                const activeCount = client.activeRequests || 0;
                if (activeCount <= 0) return null;
                const related = !focus.target || focus.relatedClientIds.has(client.id);
                const focused = focus.isFocused("client", client.id) || (focus.target?.kind === "request" && focus.relatedClientIds.has(client.id));
                const stroke = flow?.failures
                  ? "stroke-rose-300"
                  : flow?.highLatency
                    ? "stroke-amber-300"
                    : focused
                      ? "stroke-slate-400"
                      : "stroke-slate-300";
                return (
                  <g key={`flow-${client.id}`} className={cn("transition-opacity", related ? "opacity-100" : "opacity-15")}>
                    <line
                      x1={a.x}
                      y1={a.y}
                      x2={b.x}
                      y2={b.y}
                      className={stroke}
                      strokeOpacity={focused ? 0.72 : 0.52}
                      strokeWidth={focused ? 0.3 : 0.24}
                      strokeLinecap="round"
                    />
                  </g>
                );
              })
            : null}
        </svg>
          {placed.map(({ point, pos }) => {
            const isServer = point.pointType === "server";
            const related = isServer || !focus.target || focus.relatedClientIds.has(point.id);
            const focused = !isServer && focus.isFocused("client", point.id);
            return (
              <button
                type="button"
                data-map-control
                key={`${point.pointType}-${point.id}`}
                className={`absolute -translate-x-1/2 -translate-y-1/2 rounded-full outline-none transition-opacity focus-visible:ring-2 focus-visible:ring-primary/40 ${related ? "opacity-100" : "opacity-20"} ${focused ? "ring-4 ring-primary/20" : ""}`}
                style={{ left: `${pos.xPct}%`, top: `${pos.yPct}%` }}
                title={[point.label, point.city, point.region, point.country, point.activeRequests ? t("map.active", { count: point.activeRequests }) : ""].filter(Boolean).join(" · ")}
                aria-label={[isServer ? t("map.router") : point.label, point.country].filter(Boolean).join(" · ")}
                onClick={() => { if (!isServer) focus.setFocus({ kind: "client", id: point.id, source: "map" }); }}
              >
                <div
                  className={cn(
                    isServer ? "h-3 w-3 bg-primary shadow-[0_0_0_5px_rgba(0,82,255,0.10),0_8px_22px_rgba(0,82,255,0.32)]" : "h-1.5 w-1.5",
                    "rounded-full",
                    !isServer && (point.activeRequests > 0 || point.isActive ? "bg-primary opacity-100 shadow-[0_0_0_2px_rgba(0,82,255,0.16)]" : "bg-slate-500 opacity-55"),
                    point.activeRequests > 0 && "pulse-dot",
                  )}
                />
              </button>
            );
          })}
      </div>
      <div className="pointer-events-none absolute bottom-3 left-3 z-30 flex max-w-[min(46%,320px)] flex-wrap gap-2 rounded-lg border border-slate-200/70 bg-white/70 px-2 py-1.5 text-[10px] text-slate-500 backdrop-blur-md">
        <span className="inline-flex items-center gap-1"><i className="h-1.5 w-1.5 rounded-full bg-primary" />{t("map.router")}</span>
        <span className="inline-flex items-center gap-1"><i className="h-1.5 w-1.5 rounded-full bg-primary" />{t("map.activeClient")}</span>
        <span className="inline-flex items-center gap-1"><i className="h-1.5 w-1.5 rounded-full bg-slate-500 opacity-55" />{t("map.idleClient")}</span>
      </div>
      {points.length === 0 ? (
        <div className="pointer-events-none absolute inset-0 z-20 grid place-items-center text-center text-muted-foreground">
          <div>
            <div className="font-semibold text-slate-600">{t("map.waiting")}</div>
            <div className="mt-2 font-mono text-[11px] uppercase tracking-[0.14em]">{t("map.empty")}</div>
          </div>
        </div>
      ) : null}
    </section>
  );
}
