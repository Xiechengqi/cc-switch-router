import type { MapDisplaySettings, MapDisplaySettingsUpdate, MapViewportSettings } from "@/lib/types";

export const MAP_VIEWPORT_HEIGHT_PX = 420;

export const DEFAULT_MAP_VIEWPORT: MapViewportSettings = {
  visibleStartPx: 90,
};

export const DEFAULT_MAP_DISPLAY: MapDisplaySettings = {
  showFlows: true,
  showHeat: true,
  viewport: DEFAULT_MAP_VIEWPORT,
  revision: "0",
};

export function sameMapDisplaySettings(left: MapDisplaySettings, right: MapDisplaySettings) {
  return left.showFlows === right.showFlows
    && left.showHeat === right.showHeat
    && left.viewport.visibleStartPx === right.viewport.visibleStartPx;
}

export function toMapDisplayUpdate(
  settings: MapDisplaySettings,
  base?: MapDisplaySettings,
): MapDisplaySettingsUpdate {
  const update: MapDisplaySettingsUpdate = {
    expectedRevision: base?.revision ?? settings.revision,
  };
  if (!base || settings.showFlows !== base.showFlows) update.showFlows = settings.showFlows;
  if (!base || settings.showHeat !== base.showHeat) update.showHeat = settings.showHeat;
  if (!base || settings.viewport.visibleStartPx !== base.viewport.visibleStartPx) {
    update.viewport = { visibleStartPx: settings.viewport.visibleStartPx };
  }
  return update;
}

export function rebaseMapDisplaySettings(
  latest: MapDisplaySettings,
  base: MapDisplaySettings,
  draft: MapDisplaySettings,
): MapDisplaySettings {
  return {
    revision: latest.revision,
    showFlows: draft.showFlows !== base.showFlows ? draft.showFlows : latest.showFlows,
    showHeat: draft.showHeat !== base.showHeat ? draft.showHeat : latest.showHeat,
    viewport: {
      visibleStartPx: draft.viewport.visibleStartPx !== base.viewport.visibleStartPx
        ? draft.viewport.visibleStartPx
        : latest.viewport.visibleStartPx,
    },
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
