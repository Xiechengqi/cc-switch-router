"use client";

import { Minus, Plus, RotateCcw } from "lucide-react";
import { Button } from "@heroui/react";
import * as React from "react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { useDashboardFocus } from "@/components/dashboard/dashboard-focus";
import type { DashboardResponse, MapPoint, MarketRequestLog, RecentRequestEvent, ShareRequestLog } from "@/lib/types";
import { cn } from "@/lib/utils";
import { usePersistentState } from "@/lib/use-persistent-state";

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
  const dragRef = React.useRef<{ pointerId: number; x: number; y: number; panX: number; panY: number } | null>(null);
  const zoomRef = React.useRef(1);
  const [worldSvg, setWorldSvg] = React.useState("");
  const [zoom, setZoomState] = React.useState(1);
  const [pan, setPan] = React.useState({ x: 0, y: 0 });
  const [showFlows, setShowFlows] = usePersistentState("cc_switch_router_map_flows_v1", true);
  const [showHeat, setShowHeat] = usePersistentState("cc_switch_router_map_heat_v1", true);
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

  const clampPan = React.useCallback((nextPan: { x: number; y: number }, nextZoom = zoom) => {
    const shell = shellRef.current;
    if (!shell) return nextPan;
    const viewportWidth = shell.clientWidth;
    const viewportHeight = shell.clientHeight;
    const mapWidth = viewportWidth;
    const mapHeight = viewportWidth / 2;
    const maxX = Math.max(0, (mapWidth * nextZoom - viewportWidth) / 2);
    const maxY = Math.max(0, (mapHeight * nextZoom - viewportHeight) / 2);
    return {
      x: Math.max(-maxX, Math.min(maxX, nextPan.x)),
      y: Math.max(-maxY, Math.min(maxY, nextPan.y)),
    };
  }, [zoom]);

  const setZoom = React.useCallback((next: number) => {
    const nextZoom = Math.max(1, Math.min(3, Number(next.toFixed(2))));
    zoomRef.current = nextZoom;
    setZoomState(nextZoom);
    setPan((current) => clampPan(current, nextZoom));
  }, [clampPan]);

  React.useEffect(() => {
    const shell = shellRef.current;
    if (!shell) return;
    const handleWheel = (event: WheelEvent) => {
      event.preventDefault();
      setZoom(zoomRef.current + (event.deltaY < 0 ? 0.18 : -0.18));
    };
    shell.addEventListener("wheel", handleWheel, { passive: false });
    return () => shell.removeEventListener("wheel", handleWheel);
  }, [setZoom]);

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

  React.useEffect(() => {
    const root = worldRef.current;
    if (!root) return;
    const counts = showHeat ? data?.userCountryCounts || data?.countryCounts || {} : {};
    const values = Object.values(counts).filter((value) => value > 0);
    const max = values.length ? Math.max(...values) : 0;
    for (const element of Array.from(root.querySelectorAll<SVGElement>(".country"))) {
      const iso3 = Array.from(element.classList).find((name) => /^[A-Z]{3}$/.test(name));
      const count = iso3 ? counts[iso3] || 0 : 0;
      const heat = max > 0 ? Math.min(1, count / max) : 0;
      element.style.fillOpacity = String(0.1 + heat * 0.55);
      element.style.strokeOpacity = String(0.16 + heat * 0.4);
    }
  }, [data?.countryCounts, data?.userCountryCounts, showHeat, worldSvg]);

  React.useEffect(() => {
    function handleResize() {
      setPan((current) => clampPan(current));
    }
    window.addEventListener("resize", handleResize);
    return () => window.removeEventListener("resize", handleResize);
  }, [clampPan]);

  const setClampedPan = React.useCallback((nextPan: { x: number; y: number }) => {
    setPan(clampPan(nextPan));
  }, [clampPan]);

  const endDrag = React.useCallback((pointerId?: number) => {
    const shell = shellRef.current;
    if (pointerId != null) {
      try {
        shell?.releasePointerCapture(pointerId);
      } catch {
        // Pointer capture may already be released by the browser.
      }
    }
    dragRef.current = null;
  }, []);

  function reset() {
    zoomRef.current = 1;
    setZoomState(1);
    setPan({ x: 0, y: 0 });
  }

  return (
    <section
      ref={shellRef}
      className="relative h-[420px] cursor-grab select-none overflow-hidden rounded-[20px] border bg-white text-primary shadow-[0_4px_6px_rgba(15,23,42,0.04),0_12px_28px_rgba(15,23,42,0.05)] outline-none active:cursor-grabbing"
      style={{
        userSelect: "none",
        WebkitUserSelect: "none",
        WebkitTapHighlightColor: "transparent",
        touchAction: "none",
      }}
      tabIndex={0}
      aria-label={t("map.aria")}
      onDragStart={(event) => event.preventDefault()}
      onPointerDown={(event) => {
        if ((event.target as HTMLElement).closest("[data-map-control]")) return;
        event.preventDefault();
        dragRef.current = { pointerId: event.pointerId, x: event.clientX, y: event.clientY, panX: pan.x, panY: pan.y };
        shellRef.current?.setPointerCapture(event.pointerId);
      }}
      onPointerMove={(event) => {
        const drag = dragRef.current;
        if (!drag || drag.pointerId !== event.pointerId) return;
        event.preventDefault();
        setClampedPan({ x: drag.panX + event.clientX - drag.x, y: drag.panY + event.clientY - drag.y });
      }}
      onPointerUp={(event) => {
        if (dragRef.current?.pointerId === event.pointerId) endDrag(event.pointerId);
      }}
      onPointerCancel={(event) => {
        if (dragRef.current?.pointerId === event.pointerId) endDrag(event.pointerId);
      }}
      onKeyDown={(event) => {
        const step = 24;
        if (event.key === "+" || event.key === "=") setZoom(zoom + 0.25);
        else if (event.key === "-" || event.key === "_") setZoom(zoom - 0.25);
        else if (event.key === "0") reset();
        else if (event.key === "ArrowUp") setPan((p) => clampPan({ ...p, y: p.y + step }));
        else if (event.key === "ArrowDown") setPan((p) => clampPan({ ...p, y: p.y - step }));
        else if (event.key === "ArrowLeft") setPan((p) => clampPan({ ...p, x: p.x + step }));
        else if (event.key === "ArrowRight") setPan((p) => clampPan({ ...p, x: p.x - step }));
        else return;
        event.preventDefault();
      }}
    >
      <div className="pointer-events-none absolute inset-0 z-10 bg-[radial-gradient(circle,rgba(15,23,42,0.05)_1px,transparent_1px)] bg-[length:28px_28px] bg-[position:14px_14px]" />
      <div className="pointer-events-none absolute inset-0 z-10 bg-[radial-gradient(circle_at_6%_12%,rgba(0,82,255,0.10),transparent_38%),radial-gradient(circle_at_94%_88%,rgba(77,124,255,0.07),transparent_42%)]" />
      <div data-map-control className="absolute left-3 top-3 z-30 inline-flex items-center gap-1.5 rounded-full border border-slate-200/70 bg-white/70 px-2.5 py-1 backdrop-blur-md">
        <span className="live-pulse h-1.5 w-1.5 rounded-full bg-rose-500" />
        <span className="font-mono text-[9px] font-semibold uppercase tracking-[0.14em] text-slate-700">{t("map.liveActivity")}</span>
      </div>
      <RequestTicker data={data} />
      <div
        className="absolute left-1/2 top-1/2 z-20 aspect-[2/1] w-full origin-center transition-transform duration-200 ease-out"
        style={{ transform: `translate(-50%, -50%) translate(${pan.x}px, ${pan.y}px) scale(${zoom})` }}
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
      <div className="absolute bottom-3 left-3 z-30 inline-flex items-center gap-0.5 rounded-lg border border-slate-200/70 bg-white/70 p-1 text-slate-600 backdrop-blur-md">
        <Button data-map-control variant="ghost" size="sm" isIconOnly className="h-6 w-6 min-w-0 rounded-md p-0 text-slate-600 hover:bg-blue-50 hover:text-primary" aria-label={t("map.zoomOut")} onClick={() => setZoom(zoom - 0.25)}>
          <Minus className="h-3.5 w-3.5" />
        </Button>
        <span className="min-w-9 text-center font-mono text-[10px] text-slate-500">{Math.round(zoom * 100)}%</span>
        <Button data-map-control variant="ghost" size="sm" isIconOnly className="h-6 w-6 min-w-0 rounded-md p-0 text-slate-600 hover:bg-blue-50 hover:text-primary" aria-label={t("map.zoomIn")} onClick={() => setZoom(zoom + 0.25)}>
          <Plus className="h-3.5 w-3.5" />
        </Button>
        <Button data-map-control variant="ghost" size="sm" isIconOnly className="h-6 w-6 min-w-0 rounded-md p-0 text-slate-600 hover:bg-blue-50 hover:text-primary" aria-label={t("map.reset")} onClick={reset}>
          <RotateCcw className="h-3.5 w-3.5" />
        </Button>
        <span className="mx-1 h-4 w-px bg-slate-200" />
        <button data-map-control type="button" aria-pressed={showFlows} onClick={() => setShowFlows((value) => !value)} className={`rounded-md px-2 py-1 text-[10px] transition-colors ${showFlows ? "bg-primary/10 font-medium text-primary" : "hover:bg-white"}`}>{t("map.requestFlows")}</button>
        <button data-map-control type="button" aria-pressed={showHeat} onClick={() => setShowHeat((value) => !value)} className={`rounded-md px-2 py-1 text-[10px] transition-colors ${showHeat ? "bg-primary/10 font-medium text-primary" : "hover:bg-white"}`}>{t("map.demandHeat")}</button>
      </div>
      <div className="absolute bottom-3 right-3 z-30 flex max-w-[min(34%,280px)] flex-wrap gap-2 rounded-lg border border-slate-200/70 bg-white/70 px-2 py-1.5 text-[10px] text-slate-500 backdrop-blur-md">
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
