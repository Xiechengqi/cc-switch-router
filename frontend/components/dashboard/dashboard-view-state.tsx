"use client";

import * as React from "react";
import { usePersistentState } from "@/lib/use-persistent-state";

const REGION_FILTER_STORAGE_KEY = "cc_switch_router_client_regions_v2";
const LEGACY_REGION_FILTER_STORAGE_KEY = "cc_switch_router_client_region_v1";

type DashboardViewStateValue = {
  issuesOnly: boolean;
  setIssuesOnly: (value: boolean) => void;
  regionFilters: string[];
  setRegionFilters: React.Dispatch<React.SetStateAction<string[]>>;
  addRegionFilter: (region: string) => void;
  clearRegionFilters: () => void;
};

const DashboardViewStateContext = React.createContext<DashboardViewStateValue | null>(null);

function readLegacyRegionFilter(): string[] | null {
  if (typeof window === "undefined") return null;
  try {
    const stored = window.localStorage.getItem(LEGACY_REGION_FILTER_STORAGE_KEY);
    if (stored == null) return null;
    const parsed = JSON.parse(stored) as unknown;
    if (parsed === "all") return [];
    if (typeof parsed === "string" && parsed.trim()) return [parsed.trim()];
  } catch {
    // Ignore invalid legacy preferences.
  }
  return null;
}

export function DashboardViewStateProvider({ children }: { children: React.ReactNode }) {
  const [issuesOnly, setIssuesOnly] = React.useState(false);
  const [regionFilters, setRegionFilters] = usePersistentState<string[]>(REGION_FILTER_STORAGE_KEY, []);
  const migratedLegacyRegionRef = React.useRef(false);

  React.useEffect(() => {
    if (migratedLegacyRegionRef.current) return;
    migratedLegacyRegionRef.current = true;
    if (typeof window === "undefined") return;
    if (window.localStorage.getItem(REGION_FILTER_STORAGE_KEY) != null) return;
    const legacy = readLegacyRegionFilter();
    if (legacy != null) setRegionFilters(legacy);
  }, [setRegionFilters]);

  const addRegionFilter = React.useCallback((region: string) => {
    const normalized = region.trim();
    if (!normalized) return;
    setRegionFilters((current) => {
      if (current.includes(normalized)) return current;
      return [...current, normalized].sort((left, right) => left.localeCompare(right));
    });
  }, [setRegionFilters]);

  const clearRegionFilters = React.useCallback(() => {
    setRegionFilters([]);
  }, [setRegionFilters]);

  const value = React.useMemo(
    () => ({
      issuesOnly,
      setIssuesOnly,
      regionFilters,
      setRegionFilters,
      addRegionFilter,
      clearRegionFilters,
    }),
    [addRegionFilter, clearRegionFilters, issuesOnly, regionFilters, setRegionFilters],
  );

  return <DashboardViewStateContext.Provider value={value}>{children}</DashboardViewStateContext.Provider>;
}

export function useDashboardViewState() {
  const value = React.useContext(DashboardViewStateContext);
  if (!value) throw new Error("useDashboardViewState must be used inside DashboardViewStateProvider");
  return value;
}
