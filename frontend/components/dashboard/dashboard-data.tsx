"use client";

import * as React from "react";
import { useAuth } from "@/components/auth/auth-provider";
import { getDashboard } from "@/lib/api";
import type { DashboardResponse } from "@/lib/types";

type DashboardDataValue = {
  data: DashboardResponse | null;
  error: string;
  loading: boolean;
  refreshing: boolean;
  fresh: boolean;
  refresh: () => Promise<void>;
};

const DashboardDataContext = React.createContext<DashboardDataValue | null>(null);
const DASHBOARD_REFRESH_TIMEOUT_MS = 15_000;

type DashboardRefreshFlight = {
  contextKey: string;
  controller: AbortController;
  promise: Promise<void>;
};

export function DashboardDataProvider({ enabled, children }: { enabled: boolean; children: React.ReactNode }) {
  const [data, setData] = React.useState<DashboardResponse | null>(null);
  const [error, setError] = React.useState("");
  const [loading, setLoading] = React.useState(enabled);
  const [refreshing, setRefreshing] = React.useState(false);
  const [clock, setClock] = React.useState(() => Date.now());
  const requestSeq = React.useRef(0);
  const inFlightRef = React.useRef<DashboardRefreshFlight | null>(null);
  const dataRef = React.useRef<DashboardResponse | null>(null);
  const { loading: authLoading, session } = useAuth();
  const refreshContextKey = `${session?.authenticated ? "authenticated" : "anonymous"}:${session?.user?.email?.trim().toLowerCase() || "-"}`;

  const refresh = React.useCallback(() => {
    if (!enabled || authLoading) return Promise.resolve();
    const active = inFlightRef.current;
    if (active?.contextKey === refreshContextKey) return active.promise;
    active?.controller.abort();
    const controller = new AbortController();
    let timedOut = false;
    const operation = (async () => {
      const seq = ++requestSeq.current;
      const initialLoad = dataRef.current == null;
      if (initialLoad) setLoading(true);
      else setRefreshing(true);
      const timeout = window.setTimeout(() => {
        timedOut = true;
        controller.abort();
      }, DASHBOARD_REFRESH_TIMEOUT_MS);
      try {
        const next = await getDashboard(controller.signal);
        if (seq !== requestSeq.current) return;
        dataRef.current = next;
        setData(next);
        setError("");
        setClock(Date.now());
      } catch (err) {
        if (seq !== requestSeq.current) return;
        if (controller.signal.aborted && !timedOut) return;
        setError(timedOut ? "dashboard request timed out" : err instanceof Error ? err.message : String(err));
      } finally {
        window.clearTimeout(timeout);
        if (seq === requestSeq.current) {
          setLoading(false);
          setRefreshing(false);
        }
      }
    })().finally(() => {
      if (inFlightRef.current?.controller === controller) inFlightRef.current = null;
    });
    inFlightRef.current = { contextKey: refreshContextKey, controller, promise: operation };
    return operation;
  }, [authLoading, enabled, refreshContextKey]);

  React.useEffect(() => {
    requestSeq.current += 1;
    inFlightRef.current?.controller.abort();
    inFlightRef.current = null;
    dataRef.current = null;
    setData(null);
    setError("");
    setRefreshing(false);
    setLoading(enabled && !authLoading);
    setClock(Date.now());
    if (!enabled || authLoading) return;
    void refresh();
    const refreshId = window.setInterval(() => void refresh(), 5000);
    const clockId = window.setInterval(() => setClock(Date.now()), 5000);
    return () => {
      window.clearInterval(refreshId);
      window.clearInterval(clockId);
      const active = inFlightRef.current;
      if (active?.contextKey === refreshContextKey) {
        requestSeq.current += 1;
        active.controller.abort();
        inFlightRef.current = null;
      }
    };
  }, [authLoading, enabled, refresh, refreshContextKey]);

  const generatedAt = data ? Date.parse(data.generatedAt) : 0;
  const fresh = Boolean(data && !error && Number.isFinite(generatedAt) && clock - generatedAt < 20_000);
  const value = React.useMemo(
    () => ({ data, error, loading, refreshing, fresh, refresh }),
    [data, error, fresh, loading, refresh, refreshing],
  );
  return <DashboardDataContext.Provider value={value}>{children}</DashboardDataContext.Provider>;
}

export function useDashboardData() {
  const value = React.useContext(DashboardDataContext);
  if (!value) throw new Error("useDashboardData must be used inside DashboardDataProvider");
  return value;
}
