"use client";

import { usePersistentState } from "@/lib/use-persistent-state";

export const MAP_SHOW_FLOWS_STORAGE_KEY = "cc_switch_router_map_flows_v1";
export const MAP_SHOW_HEAT_STORAGE_KEY = "cc_switch_router_map_heat_v1";
export const MAP_VIEWPORT_STORAGE_KEY = "cc_switch_router_map_viewport_v1";

export type MapViewportSettings = {
  visibleStartPx: number;
  visibleEndPx: number;
  verticalPanPx: number;
};

export const DEFAULT_MAP_VIEWPORT: MapViewportSettings = {
  visibleStartPx: 74,
  visibleEndPx: 493,
  verticalPanPx: 125,
};

function clampViewport(viewport: MapViewportSettings): MapViewportSettings {
  return {
    visibleStartPx: Math.max(0, Math.min(5000, Math.round(viewport.visibleStartPx))),
    visibleEndPx: Math.max(0, Math.min(5000, Math.round(viewport.visibleEndPx))),
    verticalPanPx: Math.max(-2000, Math.min(2000, Math.round(viewport.verticalPanPx))),
  };
}

export function computeMapOffsetY(
  viewport: MapViewportSettings,
  viewportWidth: number,
  viewportHeight: number,
) {
  const mapHeight = viewportWidth / 2;
  const mapTopPx = -viewport.visibleStartPx;
  const mapBottomPx = mapTopPx + mapHeight;
  const offsetY = mapTopPx - viewportHeight / 2 + mapHeight / 2;
  if (mapBottomPx > viewport.visibleEndPx) {
    const adjustedTopPx = viewport.visibleEndPx - mapHeight;
    return adjustedTopPx - viewportHeight / 2 + mapHeight / 2 + viewport.verticalPanPx;
  }
  return offsetY + viewport.verticalPanPx;
}

export function useMapDisplaySettings() {
  const [showFlows, setShowFlows] = usePersistentState(MAP_SHOW_FLOWS_STORAGE_KEY, true);
  const [showHeat, setShowHeat] = usePersistentState(MAP_SHOW_HEAT_STORAGE_KEY, true);
  const [viewport, setViewportState] = usePersistentState(MAP_VIEWPORT_STORAGE_KEY, DEFAULT_MAP_VIEWPORT);

  const setViewport = (next: MapViewportSettings | ((prev: MapViewportSettings) => MapViewportSettings)) => {
    setViewportState((prev) => clampViewport(typeof next === "function" ? next(prev) : next));
  };

  const resetViewport = () => setViewport(DEFAULT_MAP_VIEWPORT);

  return {
    showFlows,
    setShowFlows,
    showHeat,
    setShowHeat,
    viewport: clampViewport(viewport),
    setViewport,
    resetViewport,
  };
}
