import type { MapDisplaySettings, MapDisplaySettingsUpdate, MapViewportSettings } from "@/lib/types";

export const MAP_VIEWPORT_HEIGHT_PX = 420;

export const DEFAULT_MAP_VIEWPORT: MapViewportSettings = {
  visibleStartPx: 90,
};

export const DEFAULT_MAP_DISPLAY: MapDisplaySettings = {
  showFlows: true,
  showHeat: true,
  viewport: DEFAULT_MAP_VIEWPORT,
};

export function sameMapDisplaySettings(left: MapDisplaySettings, right: MapDisplaySettings) {
  return left.showFlows === right.showFlows
    && left.showHeat === right.showHeat
    && left.viewport.visibleStartPx === right.viewport.visibleStartPx;
}

export function toMapDisplayUpdate(settings: MapDisplaySettings): MapDisplaySettingsUpdate {
  return {
    showFlows: settings.showFlows,
    showHeat: settings.showHeat,
    viewport: { visibleStartPx: settings.viewport.visibleStartPx },
  };
}

export function computeMapOffsetY(
  visibleStartPx: number,
  viewportWidth: number,
  viewportHeight: number,
) {
  const mapHeight = viewportWidth / 2;
  const mapTopPx = -visibleStartPx;
  return mapTopPx - viewportHeight / 2 + mapHeight / 2;
}
