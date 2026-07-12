"use client";

import { Loader2, RotateCcw } from "lucide-react";
import { Alert, Button, Card, Chip, Input, Switch } from "@heroui/react";
import * as React from "react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { getMapDisplay } from "@/lib/api";
import { DEFAULT_MAP_DISPLAY } from "@/lib/map-display-settings";
import type { MapDisplaySettings } from "@/lib/types";

type MapDisplayPanelProps = {
  canEdit?: boolean;
  value?: MapDisplaySettings;
  onChange?: (next: MapDisplaySettings) => void;
  dirty?: boolean;
  loading?: boolean;
};

export function MapDisplayPanel({
  canEdit = false,
  value,
  onChange,
  dirty = false,
  loading: loadingProp,
}: MapDisplayPanelProps) {
  const { t } = useLocaleText();
  const controlled = value != null && onChange != null;
  const [settings, setSettings] = React.useState<MapDisplaySettings>(value ?? DEFAULT_MAP_DISPLAY);
  const [loadingLocal, setLoadingLocal] = React.useState(!controlled);
  const [error, setError] = React.useState("");

  const display = controlled ? value : settings;
  const loading = loadingProp ?? loadingLocal;

  React.useEffect(() => {
    if (controlled) return;
    let cancelled = false;
    setLoadingLocal(true);
    getMapDisplay()
      .then((next) => {
        if (!cancelled) {
          setSettings(next);
          setError("");
        }
      })
      .catch((err) => {
        if (!cancelled) setError(err instanceof Error ? err.message : String(err));
      })
      .finally(() => {
        if (!cancelled) setLoadingLocal(false);
      });
    return () => {
      cancelled = true;
    };
  }, [controlled]);

  const updateSettings = React.useCallback((next: MapDisplaySettings | ((prev: MapDisplaySettings) => MapDisplaySettings)) => {
    if (controlled) {
      onChange(typeof next === "function" ? next(value) : next);
      return;
    }
    setSettings((prev) => (typeof next === "function" ? next(prev) : next));
  }, [controlled, onChange, value]);

  const resetViewport = React.useCallback(() => {
    updateSettings((prev) => ({ ...prev, viewport: { ...DEFAULT_MAP_DISPLAY.viewport } }));
  }, [updateSettings]);

  return (
    <Card className="rounded-lg">
      <Card.Header>
        <div className="flex flex-wrap items-center gap-2">
          <Card.Title>{t("settings.mapTitle")}</Card.Title>
          {dirty ? <Chip color="accent" size="sm" variant="soft">{t("common.changed")}</Chip> : null}
        </div>
        <Card.Description>{t("settings.mapDescription")}</Card.Description>
      </Card.Header>
      <Card.Content className="grid gap-4">
        {error ? <Alert status="danger">{error}</Alert> : null}
        {loading ? (
          <div className="flex items-center gap-2 text-sm text-muted-foreground">
            <Loader2 className="h-4 w-4 animate-spin" />
            {t("settings.loading")}
          </div>
        ) : (
          <>
            <label className="flex items-center justify-between gap-4 rounded-lg border bg-background px-4 py-3">
              <div className="grid gap-1">
                <span className="text-sm font-medium">{t("map.requestFlows")}</span>
                <span className="text-sm text-muted-foreground">{t("settings.mapShowFlowsDescription")}</span>
              </div>
              <Switch
                isSelected={display.showFlows}
                isDisabled={!canEdit}
                onChange={(next) => updateSettings((prev) => ({ ...prev, showFlows: next }))}
                aria-label={t("map.requestFlows")}
              />
            </label>
            <label className="flex items-center justify-between gap-4 rounded-lg border bg-background px-4 py-3">
              <div className="grid gap-1">
                <span className="text-sm font-medium">{t("map.demandHeat")}</span>
                <span className="text-sm text-muted-foreground">{t("settings.mapShowHeatDescription")}</span>
              </div>
              <Switch
                isSelected={display.showHeat}
                isDisabled={!canEdit}
                onChange={(next) => updateSettings((prev) => ({ ...prev, showHeat: next }))}
                aria-label={t("map.demandHeat")}
              />
            </label>

            <div className="grid gap-4 rounded-lg border bg-background px-4 py-4">
              <div className="flex flex-wrap items-start justify-between gap-3">
                <div className="grid gap-1">
                  <span className="text-sm font-medium">{t("settings.mapViewportTitle")}</span>
                  <span className="text-sm text-muted-foreground">{t("settings.mapViewportDescription")}</span>
                </div>
                {canEdit ? (
                  <Button
                    variant="outline"
                    size="sm"
                    onPress={resetViewport}
                    isDisabled={
                      display.viewport.visibleStartPx === DEFAULT_MAP_DISPLAY.viewport.visibleStartPx
                      && display.viewport.visibleEndPx === DEFAULT_MAP_DISPLAY.viewport.visibleEndPx
                      && display.viewport.verticalPanPx === DEFAULT_MAP_DISPLAY.viewport.verticalPanPx
                    }
                  >
                    <RotateCcw className="h-3.5 w-3.5" />
                    {t("settings.mapViewportReset")}
                  </Button>
                ) : null}
              </div>
              <div className="grid gap-3 md:grid-cols-3">
                <ViewportField
                  id="map-visible-start"
                  label={t("settings.mapVisibleStartPx")}
                  description={t("settings.mapVisibleStartPxDescription")}
                  value={display.viewport.visibleStartPx}
                  disabled={!canEdit}
                  onChange={(next) => updateSettings((prev) => ({
                    ...prev,
                    viewport: { ...prev.viewport, visibleStartPx: next },
                  }))}
                />
                <ViewportField
                  id="map-visible-end"
                  label={t("settings.mapVisibleEndPx")}
                  description={t("settings.mapVisibleEndPxDescription")}
                  value={display.viewport.visibleEndPx}
                  disabled={!canEdit}
                  onChange={(next) => updateSettings((prev) => ({
                    ...prev,
                    viewport: { ...prev.viewport, visibleEndPx: next },
                  }))}
                />
                <ViewportField
                  id="map-vertical-pan"
                  label={t("settings.mapVerticalPanPx")}
                  description={t("settings.mapVerticalPanPxDescription")}
                  value={display.viewport.verticalPanPx}
                  disabled={!canEdit}
                  onChange={(next) => updateSettings((prev) => ({
                    ...prev,
                    viewport: { ...prev.viewport, verticalPanPx: next },
                  }))}
                />
              </div>
            </div>
          </>
        )}
      </Card.Content>
    </Card>
  );
}

function ViewportField({
  id,
  label,
  description,
  value,
  disabled,
  onChange,
}: {
  id: string;
  label: string;
  description: string;
  value: number;
  disabled: boolean;
  onChange: (value: number) => void;
}) {
  const [draft, setDraft] = React.useState(String(value));

  React.useEffect(() => {
    setDraft(String(value));
  }, [value]);

  const commit = React.useCallback(() => {
    const parsed = Number(draft);
    if (!Number.isFinite(parsed)) {
      setDraft(String(value));
      return;
    }
    if (parsed !== value) onChange(parsed);
  }, [draft, onChange, value]);

  return (
    <label className="grid gap-2" htmlFor={id}>
      <span className="text-sm font-medium">{label}</span>
      <Input
        id={id}
        type="number"
        value={draft}
        disabled={disabled}
        onChange={(event) => setDraft(event.target.value)}
        onBlur={commit}
        onKeyDown={(event) => {
          if (event.key === "Enter") {
            event.preventDefault();
            commit();
          }
        }}
      />
      <span className="text-xs text-muted-foreground">{description}</span>
    </label>
  );
}
