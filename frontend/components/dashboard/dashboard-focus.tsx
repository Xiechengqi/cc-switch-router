"use client";

import * as React from "react";
import type { DashboardResponse } from "@/lib/types";
import { recordDashboardUxEvent } from "@/lib/api";

export type DashboardFocusKind = "request" | "client" | "share" | "market" | "country";
export type DashboardFocusSource = "map" | "client-board" | "market-table" | "drawer" | "activity";
export type DashboardFocusTarget = { kind: DashboardFocusKind; id: string; source: DashboardFocusSource };

type DashboardFocusValue = {
  target: DashboardFocusTarget | null;
  relatedClientIds: ReadonlySet<string>;
  relatedShareIds: ReadonlySet<string>;
  relatedMarketIds: ReadonlySet<string>;
  label: string;
  drawerTarget: { kind: "client" | "share" | "market"; id: string } | null;
  setFocus: (target: DashboardFocusTarget) => void;
  clearFocus: () => void;
  openDrawer: (kind: "client" | "share" | "market", id: string) => void;
  closeDrawer: () => void;
  isFocused: (kind: DashboardFocusKind, id: string) => boolean;
  isRelated: (kind: Exclude<DashboardFocusKind, "request">, id: string) => boolean;
};

const DashboardFocusContext = React.createContext<DashboardFocusValue | null>(null);

function targetFromUrl(): DashboardFocusTarget | null {
  if (typeof window === "undefined") return null;
  const params = new URLSearchParams(window.location.search);
  const kind = params.get("focusKind") as DashboardFocusKind | null;
  const id = params.get("focusId") || "";
  if (!id || !kind || !["request", "client", "share", "market", "country"].includes(kind)) return null;
  return { kind, id, source: "activity" };
}

function syncTargetToUrl(target: DashboardFocusTarget | null) {
  const url = new URL(window.location.href);
  if (target) {
    url.searchParams.set("focusKind", target.kind);
    url.searchParams.set("focusId", target.id);
  } else {
    url.searchParams.delete("focusKind");
    url.searchParams.delete("focusId");
  }
  window.history.replaceState(window.history.state, "", `${url.pathname}${url.search}${url.hash}`);
}

function drawerFromUrl() {
  if (typeof window === "undefined") return null;
  const params = new URLSearchParams(window.location.search);
  const kind = params.get("drawerKind") as "client" | "share" | "market" | null;
  const id = params.get("drawerId") || "";
  return kind && id && ["client", "share", "market"].includes(kind) ? { kind, id } : null;
}

function syncDrawerToUrl(target: { kind: "client" | "share" | "market"; id: string } | null) {
  const url = new URL(window.location.href);
  if (target) {
    url.searchParams.set("drawerKind", target.kind);
    url.searchParams.set("drawerId", target.id);
  } else {
    url.searchParams.delete("drawerKind");
    url.searchParams.delete("drawerId");
  }
  window.history.replaceState(window.history.state, "", `${url.pathname}${url.search}${url.hash}`);
}

function requestRelations(data: DashboardResponse, requestId: string) {
  const event = data.recentRequestEvents?.find((item) => item.requestId === requestId);
  const marketLog = data.marketRequestLogs?.find((item) => item.requestId === requestId);
  return {
    shareId: event?.shareId || marketLog?.shareId,
    marketId: marketLog?.marketId,
  };
}

function focusExists(data: DashboardResponse, target: DashboardFocusTarget) {
  if (target.kind === "client") return data.clients.some((client) => client.installation.id === target.id);
  if (target.kind === "share") return (data.shares || []).some((share) => share.shareId === target.id);
  if (target.kind === "market") return (data.markets || []).some((market) => market.id === target.id);
  if (target.kind === "country") {
    return Boolean(
      data.countryBoards?.[target.id]
        || data.map?.countries?.some((country) => country.countryCodeIso3 === target.id),
    );
  }
  return Boolean(data.recentRequestEvents?.some((event) => event.requestId === target.id) || data.marketRequestLogs?.some((log) => log.requestId === target.id));
}

export function DashboardFocusProvider({ data, children }: { data: DashboardResponse | null; children: React.ReactNode }) {
  const [target, setTarget] = React.useState<DashboardFocusTarget | null>(null);
  const [drawerTarget, setDrawerTarget] = React.useState<{ kind: "client" | "share" | "market"; id: string } | null>(null);
  const restoredRef = React.useRef(false);

  React.useEffect(() => {
    if (restoredRef.current) return;
    restoredRef.current = true;
    setTarget(targetFromUrl());
    setDrawerTarget(drawerFromUrl());
  }, []);

  const setFocus = React.useCallback((next: DashboardFocusTarget) => {
    setTarget(next);
    syncTargetToUrl(next);
    void recordDashboardUxEvent({ eventType: next.kind === "request" && next.source === "activity" ? "map_request_selected" : "dashboard_focus_set", source: next.source, targetType: next.kind });
  }, []);
  const clearFocus = React.useCallback(() => {
    setTarget(null);
    syncTargetToUrl(null);
    void recordDashboardUxEvent({ eventType: "dashboard_focus_clear" });
  }, []);
  const openDrawer = React.useCallback((kind: "client" | "share" | "market", id: string) => {
    const next = { kind, id };
    setDrawerTarget(next);
    syncDrawerToUrl(next);
  }, []);
  const closeDrawer = React.useCallback(() => {
    setDrawerTarget(null);
    syncDrawerToUrl(null);
  }, []);

  React.useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape" && target) clearFocus();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [clearFocus, target]);

  React.useEffect(() => {
    if (!data || !target || focusExists(data, target)) return;
    clearFocus();
  }, [clearFocus, data, target]);

  React.useEffect(() => {
    if (!data || !drawerTarget) return;
    const exists = focusExists(data, { ...drawerTarget, source: "drawer" });
    if (!exists) closeDrawer();
  }, [closeDrawer, data, drawerTarget]);

  const relations = React.useMemo(() => {
    const clients = new Set<string>();
    const shares = new Set<string>();
    const markets = new Set<string>();
    if (!data || !target) return { clients, shares, markets };

    let focusShareId: string | undefined;
    if (target.kind === "client") {
      clients.add(target.id);
      data.clients.find((client) => client.installation.id === target.id)?.shareIds?.forEach((id) => shares.add(id));
    } else if (target.kind === "share") {
      focusShareId = target.id;
      shares.add(target.id);
    } else if (target.kind === "market") {
      markets.add(target.id);
      data.markets?.find((market) => market.id === target.id)?.linkedShares?.forEach((share) => shares.add(share.shareId));
    } else if (target.kind === "country") {
      const board = data.countryBoards?.[target.id];
      board?.clientIds.forEach((clientId) => clients.add(clientId));
      board?.clients.forEach((client) => {
        clients.add(client.installationId);
        client.shares.forEach((share) => shares.add(share.shareId));
      });
      data.map?.countries
        ?.find((country) => country.countryCodeIso3 === target.id)
        ?.clientIds.forEach((clientId) => clients.add(clientId));
    } else {
      const request = requestRelations(data, target.id);
      focusShareId = request.shareId;
      if (request.marketId) markets.add(request.marketId);
    }

    if (focusShareId) shares.add(focusShareId);
    for (const client of data.clients) {
      if ((client.shareIds || []).some((shareId) => shares.has(shareId))) clients.add(client.installation.id);
    }
    for (const share of data.shares || []) {
      if (!shares.has(share.shareId)) continue;
      for (const market of share.marketLinks || []) markets.add(market.id);
    }
    for (const market of data.markets || []) {
      if (market.linkedShares?.some((share) => shares.has(share.shareId))) markets.add(market.id);
    }
    return { clients, shares, markets };
  }, [data, target]);

  const label = React.useMemo(() => {
    if (!data || !target) return "";
    if (target.kind === "client") {
      const client = data.clients.find((item) => item.installation.id === target.id);
      return client?.clientTunnel?.subdomain || client?.installation.id || target.id;
    }
    if (target.kind === "share") {
      const share = data.shares?.find((item) => item.shareId === target.id);
      return share?.subdomain || share?.shareId || target.id;
    }
    if (target.kind === "market") {
      const market = data.markets?.find((item) => item.id === target.id);
      return market?.displayName || market?.subdomain || target.id;
    }
    if (target.kind === "country") {
      return data.countryBoards?.[target.id]?.countryName
        || data.map?.countries?.find((country) => country.countryCodeIso3 === target.id)?.countryName
        || target.id;
    }
    return target.id.slice(0, 12);
  }, [data, target]);

  const value = React.useMemo<DashboardFocusValue>(() => ({
    target,
    relatedClientIds: relations.clients,
    relatedShareIds: relations.shares,
    relatedMarketIds: relations.markets,
    label,
    drawerTarget,
    setFocus,
    clearFocus,
    openDrawer,
    closeDrawer,
    isFocused: (kind, id) => target?.kind === kind && target.id === id,
    isRelated: (kind, id) => kind === "client" ? relations.clients.has(id) : kind === "share" ? relations.shares.has(id) : relations.markets.has(id),
  }), [clearFocus, closeDrawer, drawerTarget, label, openDrawer, relations.clients, relations.markets, relations.shares, setFocus, target]);

  return <DashboardFocusContext.Provider value={value}>{children}</DashboardFocusContext.Provider>;
}

export function useDashboardFocus() {
  const value = React.useContext(DashboardFocusContext);
  if (!value) throw new Error("useDashboardFocus must be used inside DashboardFocusProvider");
  return value;
}
