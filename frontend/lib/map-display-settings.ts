"use client";

import { usePersistentState } from "@/lib/use-persistent-state";

export const MAP_SHOW_FLOWS_STORAGE_KEY = "cc_switch_router_map_flows_v1";
export const MAP_SHOW_HEAT_STORAGE_KEY = "cc_switch_router_map_heat_v1";
export const MAP_DRAG_OFFSET_Y_STORAGE_KEY = "cc_switch_router_map_drag_y_v1";

export function useMapDisplaySettings() {
  const [showFlows, setShowFlows] = usePersistentState(MAP_SHOW_FLOWS_STORAGE_KEY, true);
  const [showHeat, setShowHeat] = usePersistentState(MAP_SHOW_HEAT_STORAGE_KEY, true);
  return { showFlows, setShowFlows, showHeat, setShowHeat };
}

export function useMapDragOffsetY() {
  const [dragOffsetY, setDragOffsetY] = usePersistentState(MAP_DRAG_OFFSET_Y_STORAGE_KEY, 0);
  return { dragOffsetY, setDragOffsetY };
}
