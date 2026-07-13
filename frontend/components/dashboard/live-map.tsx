"use client";

import * as React from "react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { recordDashboardUxEvent } from "@/lib/api";
import { useDashboardFocus } from "@/components/dashboard/dashboard-focus";
import { MapCountryTooltip } from "@/components/dashboard/map-country-tooltip";
import type { CountryMapPoint, DashboardResponse, MapPoint, MarketRequestLog, ShareRequestLog } from "@/lib/types";
import { cn } from "@/lib/utils";
import { computeMapOffsetY, DEFAULT_MAP_DISPLAY, MAP_VIEWPORT_HEIGHT_PX } from "@/lib/map-display-settings";
import { StatsStrip } from "@/components/dashboard/stats-strip";

type PlacedPoint = { x: number; y: number; xPct: number; yPct: number };
type TickerMeta = Partial<Omit<ShareRequestLog, "createdAt"> & Omit<MarketRequestLog, "createdAt">> & {
  createdAt?: string | number;
  shareName?: string;
  userCountry?: string;
  userCountryIso3?: string;
};

const REQUEST_TICKER_LIMIT = 6;

function projectLatLon(lat: number, lon: number): PlacedPoint {
  const x = ((lon + 180) / 360) * 100;
  const y = ((90 - lat) / 180) * 100;
  const xPct = Math.max(1, Math.min(99, x));
  const yPct = Math.max(1, Math.min(99, y));
  return { x: xPct * 3.6, y: yPct * 1.8, xPct, yPct };
}

function projectPoint(point: MapPoint) {
  if (typeof point.lat !== "number" || typeof point.lon !== "number") return null;
  return projectLatLon(point.lat, point.lon);
}

function displayCountry(...values: Array<string | undefined | null>) {
  for (const value of values) {
    const trimmed = String(value || "").trim();
    if (trimmed && trimmed !== "-" && trimmed !== "--") return trimmed;
  }
  return "--";
}

function shouldIgnoreMapRowClick() {
  const selection = window.getSelection();
  return Boolean(selection && !selection.isCollapsed && selection.toString().trim());
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
  return [meta?.userEmail || "", modelName, String(status), latency, tokens, fee].filter(Boolean).join(" · ");
}

function resolveMapHeatCounts(data: DashboardResponse | null, showHeat: boolean) {
  if (!showHeat || !data) return {};
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

function resolveIso3FromElement(element: Element | null) {
  if (!element) return null;
  const countryElement = element.closest(".country");
  if (!countryElement) return null;
  return Array.from(countryElement.classList).find((name) => /^[A-Z]{3}$/.test(name)) || null;
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
        const country = displayCountry(event.userCountry, mergedItem?.userCountry, event.countryCode, mergedItem?.userCountryIso3);
        const subdomain = event.shareSubdomain || event.subdomain || event.shareName || mergedItem?.shareName || "share";
        const eventKey = [event.requestId, event.startedAt || event.createdAt || ""].join(":");
        const statusCode = Number(mergedItem?.statusCode || 0);
        const rawStatus = String(mergedItem?.status || event.healthStatus || "").toLowerCase();
        const failed = statusCode >= 400 || ["failed", "error", "offline"].includes(rawStatus);
        const badge = event.isHealthCheck ? "HC" : statusCode ? String(statusCode) : rawStatus ? rawStatus.slice(0, 3).toUpperCase() : "—";
        return (
          <div
            role="button"
            tabIndex={0}
            data-map-control
            key={eventKey}
            onClick={(clickEvent) => {
              if (shouldIgnoreMapRowClick()) return;
              focus.setFocus({ kind: "request", id: event.requestId, source: "activity" });
            }}
            onKeyDown={(keyEvent) => {
              if (keyEvent.key !== "Enter" && keyEvent.key !== " ") return;
              keyEvent.preventDefault();
              focus.setFocus({ kind: "request", id: event.requestId, source: "activity" });
            }}
            className={`pointer-events-auto flex max-w-full select-text cursor-pointer items-center gap-2 overflow-hidden rounded-lg border px-2.5 py-1.5 text-left text-[10px] text-slate-700 backdrop-blur-md transition-colors ${index === events.length - 1 ? "activity-feed-enter" : ""} ${focus.isFocused("request", event.requestId) ? "border-primary bg-white ring-2 ring-primary/20" : "border-slate-200/80 bg-white/75 hover:bg-white"}`}
          >
            <span className="select-text font-mono text-slate-500">{formatTickerTime(event.startedAt || event.createdAt, item?.createdAt)}</span>
            <span className={`inline-flex h-[15px] shrink-0 select-none items-center rounded px-1.5 font-mono text-[9px] font-semibold ${event.isHealthCheck ? "bg-blue-100 text-blue-700" : failed ? "bg-rose-100 text-rose-700" : "bg-emerald-100 text-emerald-700"}`}>{badge}</span>
            <span className="min-w-0 select-text truncate text-[11px] text-slate-700"><strong className="font-semibold">{subdomain}</strong> · {countryFlag(country)} {country} · {tickerDetail(mergedItem)}</span>
          </div>
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
  const [hoveredIso3, setHoveredIso3] = React.useState<string | null>(null);
  const [pinnedIso3, setPinnedIso3] = React.useState<string | null>(null);
  const [tooltipPos, setTooltipPos] = React.useState({ x: 0, y: 0 });
  const mapDisplay = data?.mapDisplay ?? DEFAULT_MAP_DISPLAY;
  const { showFlows, showHeat, viewport } = mapDisplay;
  const countries = data?.map?.countries || [];
  const server = data?.map?.server;
  const activeIso3 = pinnedIso3 || hoveredIso3;
  const activeBoard = activeIso3 ? data?.countryBoards?.[activeIso3] : undefined;

  const countryPlaced = React.useMemo(
    () =>
      countries.map((country) => ({
        country,
        pos: projectLatLon(country.lat, country.lon),
      })),
    [countries],
  );
  const serverPlaced = React.useMemo(() => {
    if (!server) return null;
    const pos = projectPoint(server);
    return pos ? { point: server, pos } : null;
  }, [server]);

  const requestFlows = React.useMemo(() => {
    const shareToIso3 = new Map<string, string>();
    for (const client of data?.clients || []) {
      const iso3 = countries.find((country) => country.clientIds.includes(client.installation.id))?.countryCodeIso3;
      if (!iso3) continue;
      for (const shareId of client.shareIds || []) {
        shareToIso3.set(shareId, iso3);
      }
    }
    const meta = buildRequestMeta(data);
    const flows = new Map<string, { inflight: number; failures: number; highLatency: number }>();
    for (const event of (data?.recentRequestEvents || []).slice(-200)) {
      if (!event.isInflight) continue;
      const iso3 = event.shareId ? shareToIso3.get(event.shareId) : undefined;
      if (!iso3) continue;
      const item = meta.get(event.requestId);
      const statusCode = Number(item?.statusCode || 0);
      const status = String(item?.status || event.healthStatus || "").toLowerCase();
      const latency = Number(event.latencyMs || item?.latencyMs || 0);
      const flow = flows.get(iso3) || { inflight: 0, failures: 0, highLatency: 0 };
      flow.inflight += 1;
      if (statusCode >= 400 || ["failed", "error", "offline"].includes(status)) flow.failures += 1;
      if (latency >= 2000) flow.highLatency += 1;
      flows.set(iso3, flow);
    }
    return flows;
  }, [countries, data]);

  const isCountryRelated = React.useCallback(
    (country: CountryMapPoint) => {
      if (!focus.target) return true;
      if (focus.target.kind === "country") return focus.target.id === country.countryCodeIso3;
      return country.clientIds.some((clientId) => focus.relatedClientIds.has(clientId));
    },
    [focus.relatedClientIds, focus.target],
  );

  const isCountryFocused = React.useCallback(
    (country: CountryMapPoint) =>
      focus.target?.kind === "country" && focus.target.id === country.countryCodeIso3,
    [focus.target],
  );

  React.useLayoutEffect(() => {
    const shell = shellRef.current;
    if (!shell) return;
    const updateOffset = () => {
      const height = shell.clientHeight || MAP_VIEWPORT_HEIGHT_PX;
      setMapOffsetY(computeMapOffsetY(viewport.visibleStartPx, shell.clientWidth, height));
    };
    updateOffset();
    const observer = new ResizeObserver(updateOffset);
    observer.observe(shell);
    return () => observer.disconnect();
  }, [viewport.visibleStartPx]);

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
      const heat = max > 0 ? Math.min(1, Math.sqrt(count / max)) : 0;
      const fillOpacity = String(0.06 + heat * 0.74);
      const strokeOpacity = String(0.12 + heat * 0.48);
      const highlighted = iso3 && (iso3 === activeIso3 || isCountryFocused({ countryCodeIso3: iso3 } as CountryMapPoint));
      if (!element.style.transition) {
        element.style.transition = "fill-opacity 0.8s ease-out, stroke-opacity 0.8s ease-out, stroke-width 0.2s ease-out";
      }
      element.style.pointerEvents = count > 0 ? "auto" : "none";
      element.style.cursor = count > 0 ? "pointer" : "default";
      element.style.fillOpacity = fillOpacity;
      element.style.strokeOpacity = highlighted ? "0.92" : strokeOpacity;
      element.style.strokeWidth = highlighted ? "0.55" : "0.28";
    }
  }, [activeIso3, heatCountsKey, isCountryFocused, worldSvg]);

  React.useEffect(() => {
    const root = worldRef.current;
    if (!root) return;

    const updateTooltipPosition = (event: MouseEvent) => {
      const shell = shellRef.current;
      if (!shell) return;
      const rect = shell.getBoundingClientRect();
      setTooltipPos({
        x: Math.min(Math.max(12, event.clientX - rect.left + 12), rect.width - 24),
        y: Math.min(Math.max(12, event.clientY - rect.top + 12), rect.height - 24),
      });
    };

    const onPointerOver = (event: PointerEvent) => {
      const iso3 = resolveIso3FromElement(event.target as Element);
      if (!iso3 || !(JSON.parse(heatCountsKey) as Record<string, number>)[iso3]) return;
      setHoveredIso3(iso3);
      updateTooltipPosition(event);
    };
    const onPointerMove = (event: PointerEvent) => {
      if (!hoveredIso3 && !pinnedIso3) return;
      updateTooltipPosition(event);
    };
    const onPointerLeave = (event: PointerEvent) => {
      const next = resolveIso3FromElement(event.relatedTarget as Element | null);
      if (next) return;
      setHoveredIso3(null);
    };
    const onClick = (event: MouseEvent) => {
      const iso3 = resolveIso3FromElement(event.target as Element);
      if (!iso3 || !(JSON.parse(heatCountsKey) as Record<string, number>)[iso3]) return;
      setPinnedIso3((current) => (current === iso3 ? null : iso3));
      focus.setFocus({ kind: "country", id: iso3, source: "map" });
      void recordDashboardUxEvent({ eventType: "country_located_from_map", source: "map", targetType: "country" });
    };

    root.addEventListener("pointerover", onPointerOver);
    root.addEventListener("pointermove", onPointerMove);
    root.addEventListener("pointerleave", onPointerLeave);
    root.addEventListener("click", onClick);
    return () => {
      root.removeEventListener("pointerover", onPointerOver);
      root.removeEventListener("pointermove", onPointerMove);
      root.removeEventListener("pointerleave", onPointerLeave);
      root.removeEventListener("click", onClick);
    };
  }, [focus, heatCountsKey, hoveredIso3, pinnedIso3]);

  React.useEffect(() => {
    if (!focus.target) setPinnedIso3(null);
  }, [focus.target]);

  return (
    <section
      ref={shellRef}
      className="relative h-[420px] overflow-hidden rounded-[20px] border bg-white text-primary shadow-[0_4px_6px_rgba(15,23,42,0.04),0_12px_28px_rgba(15,23,42,0.05)]"
      aria-label={t("map.aria")}
    >
      <div className="pointer-events-none absolute inset-0 z-10 bg-[radial-gradient(circle,rgba(15,23,42,0.05)_1px,transparent_1px)] bg-[length:28px_28px] bg-[position:14px_14px]" />
      <div className="pointer-events-none absolute inset-0 z-10 bg-[radial-gradient(circle_at_6%_12%,rgba(0,82,255,0.10),transparent_38%),radial-gradient(circle_at_94%_88%,rgba(77,124,255,0.07),transparent_42%)]" />
      <StatsStrip
        data={data}
        className="pointer-events-auto absolute left-3 top-3 z-30 max-w-[min(72%,560px)] select-text"
      />
      <RequestTicker data={data} />
      <div
        className="absolute left-1/2 top-1/2 z-20 aspect-[2/1] w-full origin-center"
        style={{ transform: `translate(-50%, -50%) translate(0px, ${mapOffsetY}px)` }}
      >
        {worldSvg ? (
          <div
            ref={worldRef}
            className="absolute inset-0 text-primary [&_svg]:absolute [&_svg]:inset-0 [&_svg]:block [&_svg]:h-full [&_svg]:w-full"
            aria-hidden="true"
            dangerouslySetInnerHTML={{ __html: worldSvg }}
          />
        ) : (
          <div className="pointer-events-none absolute inset-0 bg-[url('/world-map.svg')] bg-[length:100%_100%] bg-center bg-no-repeat" aria-hidden="true" />
        )}
        <svg className="pointer-events-none absolute inset-0 h-full w-full overflow-visible" viewBox="0 0 360 180" preserveAspectRatio="none" aria-hidden="true">
          {showFlows && serverPlaced
            ? countryPlaced.map(({ country, pos: b }) => {
                const a = serverPlaced.pos;
                const flow = requestFlows.get(country.countryCodeIso3);
                if ((country.inflightRequests || 0) <= 0) return null;
                const related = !focus.target || isCountryRelated(country);
                const focused = isCountryFocused(country) || (focus.target?.kind === "request" && related);
                const stroke = flow?.failures
                  ? "stroke-rose-300"
                  : flow?.highLatency
                    ? "stroke-amber-300"
                    : focused
                      ? "stroke-slate-400"
                      : "stroke-slate-300";
                return (
                  <g key={`flow-${country.countryCodeIso3}`} className={cn("transition-opacity", related ? "opacity-100" : "opacity-15")}>
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
        {countryPlaced.map(({ country, pos }) => {
          const related = !focus.target || isCountryRelated(country);
          const focused = isCountryFocused(country) || activeIso3 === country.countryCodeIso3;
          return (
            <button
              type="button"
              data-map-control
              key={`country-${country.countryCodeIso3}`}
              className={cn(
                "absolute -translate-x-1/2 -translate-y-1/2 rounded-full outline-none transition-all focus-visible:ring-2 focus-visible:ring-primary/40",
                related ? "opacity-100" : "opacity-20",
                focused ? "ring-4 ring-primary/20" : "",
              )}
              style={{ left: `${pos.xPct}%`, top: `${pos.yPct}%` }}
              title={[
                country.countryName || country.countryCode,
                t("map.countryClients", { count: country.clientCount }),
                country.inflightRequests ? t("map.active", { count: country.inflightRequests }) : "",
              ].filter(Boolean).join(" · ")}
              aria-label={[
                country.countryName || country.countryCode,
                t("map.countryClients", { count: country.clientCount }),
              ].filter(Boolean).join(" · ")}
              onMouseEnter={() => setHoveredIso3(country.countryCodeIso3)}
              onClick={() => {
                setPinnedIso3(country.countryCodeIso3);
                focus.setFocus({ kind: "country", id: country.countryCodeIso3, source: "map" });
                void recordDashboardUxEvent({ eventType: "country_located_from_map", source: "map", targetType: "country" });
              }}
            >
              <div
                className={cn(
                  "rounded-full bg-primary shadow-[0_0_0_2px_rgba(0,82,255,0.16)]",
                  country.clientCount > 1 ? "h-2 w-2" : "h-1.5 w-1.5",
                  country.inflightRequests > 0 ? "pulse-dot opacity-100" : "opacity-80",
                  focused && "h-2.5 w-2.5",
                )}
              />
            </button>
          );
        })}
        {serverPlaced ? (
          <button
            type="button"
            data-map-control
            className="absolute -translate-x-1/2 -translate-y-1/2 rounded-full opacity-100 outline-none focus-visible:ring-2 focus-visible:ring-primary/40"
            style={{ left: `${serverPlaced.pos.xPct}%`, top: `${serverPlaced.pos.yPct}%` }}
            title={t("map.router")}
            aria-label={t("map.router")}
          >
            <div className="h-3 w-3 rounded-full bg-primary shadow-[0_0_0_5px_rgba(0,82,255,0.10),0_8px_22px_rgba(0,82,255,0.32)]" />
          </button>
        ) : null}
      </div>
      {activeBoard ? (
        <MapCountryTooltip
          board={activeBoard}
          className="absolute z-40"
          style={{ left: tooltipPos.x, top: tooltipPos.y, transform: "translate(0, 0)" }}
        />
      ) : null}
      {countries.length === 0 && !server ? (
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
