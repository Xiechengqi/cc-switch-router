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

export function DashboardDataProvider({ enabled, children }: { enabled: boolean; children: React.ReactNode }) {
  const [data, setData] = React.useState<DashboardResponse | null>(null);
  const [error, setError] = React.useState("");
  const [loading, setLoading] = React.useState(enabled);
  const [refreshing, setRefreshing] = React.useState(false);
  const [clock, setClock] = React.useState(() => Date.now());
  const requestSeq = React.useRef(0);
  const dataRef = React.useRef<DashboardResponse | null>(null);
  const { loading: authLoading, session } = useAuth();

  const refresh = React.useCallback(async () => {
    if (!enabled || authLoading) return;
    const seq = ++requestSeq.current;
    const initialLoad = dataRef.current == null;
    if (initialLoad) setLoading(true);
    else setRefreshing(true);
    try {
      const next = await getDashboard();
      if (seq !== requestSeq.current) return;
      dataRef.current = next;
      setData(next);
      setError("");
      setClock(Date.now());
    } catch (err) {
      if (seq === requestSeq.current) setError(err instanceof Error ? err.message : String(err));
    } finally {
      if (seq === requestSeq.current) {
        setLoading(false);
        setRefreshing(false);
      }
    }
  }, [authLoading, enabled]);

  React.useEffect(() => {
    if (!enabled || authLoading) return;
    void refresh();
    const refreshId = window.setInterval(() => void refresh(), 5000);
    const clockId = window.setInterval(() => setClock(Date.now()), 5000);
    return () => {
      window.clearInterval(refreshId);
      window.clearInterval(clockId);
    };
  }, [authLoading, enabled, refresh, session?.authenticated, session?.user?.email]);

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
