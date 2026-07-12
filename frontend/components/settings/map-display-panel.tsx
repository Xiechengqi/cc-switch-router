"use client";

import { Loader2, RotateCcw } from "lucide-react";
import { Alert, Button, Card, Input, Switch } from "@heroui/react";
import * as React from "react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { getMapDisplay, updateMapDisplay } from "@/lib/api";
import { DEFAULT_MAP_DISPLAY } from "@/lib/map-display-settings";
import type { MapDisplaySettings } from "@/lib/types";

type MapDisplayPanelProps = {
  canEdit?: boolean;
};

export function MapDisplayPanel({ canEdit = false }: MapDisplayPanelProps) {
  const { t } = useLocaleText();
  const [settings, setSettings] = React.useState<MapDisplaySettings>(DEFAULT_MAP_DISPLAY);
  const [loading, setLoading] = React.useState(true);
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState("");

  React.useEffect(() => {
    let cancelled = false;
    setLoading(true);
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
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const patch = React.useCallback(async (update: Parameters<typeof updateMapDisplay>[0]) => {
    if (!canEdit) return;
    setBusy(true);
    setError("");
    try {
      const next = await updateMapDisplay(update);
      setSettings(next);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }, [canEdit]);

  const resetViewport = React.useCallback(() => {
    void patch({ viewport: DEFAULT_MAP_DISPLAY.viewport });
  }, [patch]);

  return (
    <Card className="rounded-lg">
      <Card.Header>
        <Card.Title>{t("settings.mapTitle")}</Card.Title>
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
                isSelected={settings.showFlows}
                isDisabled={!canEdit || busy}
                onChange={(value) => {
                  setSettings((prev) => ({ ...prev, showFlows: value }));
                  void patch({ showFlows: value });
                }}
                aria-label={t("map.requestFlows")}
              />
            </label>
            <label className="flex items-center justify-between gap-4 rounded-lg border bg-background px-4 py-3">
              <div className="grid gap-1">
                <span className="text-sm font-medium">{t("map.demandHeat")}</span>
                <span className="text-sm text-muted-foreground">{t("settings.mapShowHeatDescription")}</span>
              </div>
              <Switch
                isSelected={settings.showHeat}
                isDisabled={!canEdit || busy}
                onChange={(value) => {
                  setSettings((prev) => ({ ...prev, showHeat: value }));
                  void patch({ showHeat: value });
                }}
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
                      busy
                      || (
                        settings.viewport.visibleStartPx === DEFAULT_MAP_DISPLAY.viewport.visibleStartPx
                        && settings.viewport.visibleEndPx === DEFAULT_MAP_DISPLAY.viewport.visibleEndPx
                        && settings.viewport.verticalPanPx === DEFAULT_MAP_DISPLAY.viewport.verticalPanPx
                      )
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
                  value={settings.viewport.visibleStartPx}
                  disabled={!canEdit || busy}
                  onCommit={(value) => {
                    setSettings((prev) => ({ ...prev, viewport: { ...prev.viewport, visibleStartPx: value } }));
                    void patch({ viewport: { visibleStartPx: value } });
                  }}
                />
                <ViewportField
                  id="map-visible-end"
                  label={t("settings.mapVisibleEndPx")}
                  description={t("settings.mapVisibleEndPxDescription")}
                  value={settings.viewport.visibleEndPx}
                  disabled={!canEdit || busy}
                  onCommit={(value) => {
                    setSettings((prev) => ({ ...prev, viewport: { ...prev.viewport, visibleEndPx: value } }));
                    void patch({ viewport: { visibleEndPx: value } });
                  }}
                />
                <ViewportField
                  id="map-vertical-pan"
                  label={t("settings.mapVerticalPanPx")}
                  description={t("settings.mapVerticalPanPxDescription")}
                  value={settings.viewport.verticalPanPx}
                  disabled={!canEdit || busy}
                  onCommit={(value) => {
                    setSettings((prev) => ({ ...prev, viewport: { ...prev.viewport, verticalPanPx: value } }));
                    void patch({ viewport: { verticalPanPx: value } });
                  }}
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
  onCommit,
}: {
  id: string;
  label: string;
  description: string;
  value: number;
  disabled: boolean;
  onCommit: (value: number) => void;
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
    if (parsed !== value) onCommit(parsed);
  }, [draft, onCommit, value]);

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
