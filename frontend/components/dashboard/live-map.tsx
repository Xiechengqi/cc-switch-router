"use client";

import { Minus, Plus, RotateCcw } from "lucide-react";
import { Button } from "@heroui/react";
import * as React from "react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import type { DashboardResponse, MapPoint, MarketRequestLog, RecentRequestEvent, ShareRequestLog } from "@/lib/types";
import { cn } from "@/lib/utils";

function projectPoint(point: MapPoint) {
  if (typeof point.lat !== "number" || typeof point.lon !== "number") return null;
  const x = ((point.lon + 180) / 360) * 100;
  const y = ((90 - point.lat) / 180) * 100;
  return { x: Math.max(1, Math.min(99, x)), y: Math.max(1, Math.min(99, y)) };
}

function countryFlag(code?: string) {
  const cc = (code || "").trim().slice(0, 2).toUpperCase();
  if (!/^[A-Z]{2}$/.test(cc)) return "·";
  return String.fromCodePoint(...[...cc].map((ch) => 127397 + ch.charCodeAt(0)));
}

function formatTickerTime(value?: string | number) {
  const date = value ? new Date(value) : new Date();
  if (!Number.isFinite(date.getTime())) return "--:--:--";
  return date.toISOString().slice(11, 19);
}

function totalTokens(log?: Partial<ShareRequestLog | MarketRequestLog> | null) {
  return Number(log?.inputTokens || 0) + Number(log?.outputTokens || 0);
}

function tickerDetail(event: RecentRequestEvent, meta?: Partial<ShareRequestLog | MarketRequestLog>) {
  const agent = "requestAgent" in (meta || {}) ? meta?.requestAgent : "";
  const requested = "requestedModel" in (meta || {}) ? meta?.requestedModel : "";
  const actual = "actualModel" in (meta || {}) ? meta?.actualModel : "";
  const model = actual || requested || ("model" in (meta || {}) ? meta?.model : "") || "-";
  const status = "statusCode" in (meta || {}) ? meta?.statusCode : undefined;
  const latency = "latencyMs" in (meta || {}) ? meta?.latencyMs : undefined;
  const parts = [
    [agent, requested && actual && requested !== actual ? `${requested} -> ${actual}` : model].filter(Boolean).join(" · "),
    status ? String(status) : "",
    latency ? `${latency}ms` : "",
    totalTokens(meta) ? `${totalTokens(meta)} tok` : "",
  ].filter(Boolean);
  return parts.join(" · ") || event.shareName || event.subdomain || "request";
}

function buildRequestMeta(data: DashboardResponse | null) {
  const meta = new Map<string, Partial<ShareRequestLog | MarketRequestLog>>();
  for (const share of data?.tickerShares || []) {
    for (const log of share.recentRequests || []) {
      meta.set(log.requestId, { ...log, shareName: share.shareName, shareId: share.shareId });
    }
  }
  for (const client of data?.clients || []) {
    const share = client.share;
    for (const log of share?.recentRequests || []) {
      meta.set(log.requestId, { ...log, shareName: share?.shareName || log.shareName, shareId: share?.shareId || log.shareId });
    }
  }
  for (const log of data?.marketRequestLogs || []) {
    meta.set(log.requestId, { ...(meta.get(log.requestId) || {}), ...log });
  }
  return meta;
}

function RequestTicker({ data }: { data: DashboardResponse | null }) {
  const meta = React.useMemo(() => buildRequestMeta(data), [data]);
  const events = React.useMemo(() => {
    return [...(data?.recentRequestEvents || [])]
      .sort((a, b) => new Date(b.startedAt || b.createdAt || 0).getTime() - new Date(a.startedAt || a.createdAt || 0).getTime())
      .slice(0, 6);
  }, [data]);

  if (!events.length) return null;

  return (
    <div className="absolute left-[1.6%] top-[3.5%] z-20 flex max-w-[min(68%,760px)] flex-col items-start gap-1.5">
      {events.map((event) => {
        const item = meta.get(event.requestId);
        return (
          <div key={event.requestId} className="flex max-w-full items-center gap-1 overflow-hidden rounded-md border border-slate-200/70 bg-white/55 px-2 py-1 text-[10px] text-slate-700 backdrop-blur-sm">
            <span className="font-mono text-slate-500">{formatTickerTime(event.startedAt || event.createdAt)}</span>
            <span>{countryFlag(event.countryCode)}</span>
            <span className="font-semibold text-slate-600">{event.countryCode || "??"}</span>
            <span className="font-semibold text-slate-500">{event.subdomain || event.shareName || "-"}</span>
            <span className="truncate font-semibold text-slate-700/80">{tickerDetail(event, item)}</span>
          </div>
        );
      })}
    </div>
  );
}

export function LiveMap({ data }: { data: DashboardResponse | null }) {
  const { t } = useLocaleText();
  const shellRef = React.useRef<HTMLDivElement | null>(null);
  const worldRef = React.useRef<HTMLDivElement | null>(null);
  const dragRef = React.useRef<{ pointerId: number; x: number; y: number; panX: number; panY: number } | null>(null);
  const [worldSvg, setWorldSvg] = React.useState("");
  const [zoom, setZoomState] = React.useState(1);
  const [pan, setPan] = React.useState({ x: 0, y: 0 });
  const clients = data?.map?.clients || [];
  const server = data?.map?.server;
  const points = [server, ...clients].filter(Boolean) as MapPoint[];

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
    setZoomState(nextZoom);
    setPan((current) => clampPan(current, nextZoom));
  }, [clampPan]);

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
    const counts = data?.userCountryCounts || data?.countryCounts || {};
    const values = Object.values(counts).filter((value) => value > 0);
    const max = values.length ? Math.max(...values) : 0;
    for (const element of Array.from(root.querySelectorAll<SVGElement>(".country"))) {
      const iso3 = Array.from(element.classList).find((name) => /^[A-Z]{3}$/.test(name));
      const count = iso3 ? counts[iso3] || 0 : 0;
      const heat = max > 0 ? Math.min(1, count / max) : 0;
      element.style.fillOpacity = String(0.1 + heat * 0.55);
      element.style.strokeOpacity = String(0.16 + heat * 0.4);
    }
  }, [data?.countryCounts, data?.userCountryCounts, worldSvg]);

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
      onWheel={(event) => {
        event.preventDefault();
        setZoom(zoom + (event.deltaY < 0 ? 0.18 : -0.18));
      }}
      onPointerDown={(event) => {
        if ((event.target as HTMLElement).closest("button")) return;
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
          {server
            ? clients.map((client) => {
                const a = projectPoint(server);
                const b = projectPoint(client);
                if (!a || !b) return null;
                return (
                  <line
                    key={`flow-${client.id}`}
                    x1={a.x * 3.6}
                    y1={a.y * 1.8}
                    x2={b.x * 3.6}
                    y2={b.y * 1.8}
                    className={cn("stroke-blue-500/35", client.activeRequests > 0 ? "animate-pulse" : "stroke-slate-400/25")}
                    strokeWidth={client.activeRequests > 0 ? 0.7 : 0.5}
                    strokeDasharray={client.activeRequests > 0 ? "1.5 2.5" : "1 5"}
                    strokeLinecap="round"
                  />
                );
              })
            : null}
        </svg>
          {points.map((point) => {
            const pos = projectPoint(point);
            if (!pos) return null;
            const isServer = point.pointType === "server";
            return (
              <button
                type="button"
                key={`${point.pointType}-${point.id}`}
                className="absolute -translate-x-1/2 -translate-y-1/2 rounded-full focus:outline-none"
                style={{ left: `${pos.x}%`, top: `${pos.y}%` }}
                title={[point.label, point.city, point.region, point.country, point.activeRequests ? t("map.active", { count: point.activeRequests }) : ""].filter(Boolean).join(" · ")}
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
      <div className="absolute bottom-[4%] left-[1.6%] z-30 inline-flex items-center gap-0.5 rounded-lg border border-slate-200/70 bg-white/50 p-1 text-slate-600 backdrop-blur-sm">
        <Button variant="ghost" size="sm" isIconOnly className="h-6 w-6 min-w-0 rounded-md p-0 text-slate-600 hover:bg-blue-50 hover:text-primary" aria-label={t("map.zoomOut")} onClick={() => setZoom(zoom - 0.25)}>
          <Minus className="h-3.5 w-3.5" />
        </Button>
        <span className="min-w-9 text-center font-mono text-[10px] text-slate-500">{Math.round(zoom * 100)}%</span>
        <Button variant="ghost" size="sm" isIconOnly className="h-6 w-6 min-w-0 rounded-md p-0 text-slate-600 hover:bg-blue-50 hover:text-primary" aria-label={t("map.zoomIn")} onClick={() => setZoom(zoom + 0.25)}>
          <Plus className="h-3.5 w-3.5" />
        </Button>
        <Button variant="ghost" size="sm" isIconOnly className="h-6 w-6 min-w-0 rounded-md p-0 text-slate-600 hover:bg-blue-50 hover:text-primary" aria-label={t("map.reset")} onClick={reset}>
          <RotateCcw className="h-3.5 w-3.5" />
        </Button>
      </div>
      <div className="absolute bottom-[4%] right-[1.6%] z-30 flex max-w-[min(34%,280px)] flex-wrap gap-2 rounded-lg border border-slate-200/70 bg-white/50 px-2 py-1.5 text-[10px] text-slate-500 backdrop-blur-sm">
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
