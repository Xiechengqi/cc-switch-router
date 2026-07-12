import type { MapDisplaySettings, MapViewportSettings } from "@/lib/types";

export const DEFAULT_MAP_VIEWPORT: MapViewportSettings = {
  visibleStartPx: 74,
  visibleEndPx: 493,
  verticalPanPx: 125,
};

export const DEFAULT_MAP_DISPLAY: MapDisplaySettings = {
  showFlows: true,
  showHeat: true,
  viewport: DEFAULT_MAP_VIEWPORT,
};

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
