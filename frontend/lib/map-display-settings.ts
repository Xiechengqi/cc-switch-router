import type { MapDisplaySettings, MapDisplaySettingsUpdate, MapViewportSettings } from "@/lib/types";

export const DEFAULT_MAP_VIEWPORT: MapViewportSettings = {
  visibleStartPx: 14,
  visibleEndPx: 433,
  verticalPanPx: 0,
};

export const DEFAULT_MAP_DISPLAY: MapDisplaySettings = {
  showFlows: true,
  showHeat: true,
  viewport: DEFAULT_MAP_VIEWPORT,
};

export function sameMapDisplaySettings(left: MapDisplaySettings, right: MapDisplaySettings) {
  return left.showFlows === right.showFlows
    && left.showHeat === right.showHeat
    && left.viewport.visibleStartPx === right.viewport.visibleStartPx
    && left.viewport.visibleEndPx === right.viewport.visibleEndPx
    && left.viewport.verticalPanPx === right.viewport.verticalPanPx;
}

export function toMapDisplayUpdate(settings: MapDisplaySettings): MapDisplaySettingsUpdate {
  return {
    showFlows: settings.showFlows,
    showHeat: settings.showHeat,
    viewport: { ...settings.viewport },
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
