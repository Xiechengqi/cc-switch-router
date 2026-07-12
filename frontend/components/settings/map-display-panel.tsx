"use client";

import { RotateCcw } from "lucide-react";
import { Button, Card, Input, Switch } from "@heroui/react";
import * as React from "react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import {
  DEFAULT_MAP_VIEWPORT,
  useMapDisplaySettings,
} from "@/lib/map-display-settings";

type MapDisplayPanelProps = {
  canEditViewport?: boolean;
};

export function MapDisplayPanel({ canEditViewport = false }: MapDisplayPanelProps) {
  const { t } = useLocaleText();
  const { showFlows, setShowFlows, showHeat, setShowHeat, viewport, setViewport, resetViewport } = useMapDisplaySettings();

  return (
    <Card className="rounded-lg">
      <Card.Header>
        <Card.Title>{t("settings.mapTitle")}</Card.Title>
        <Card.Description>{t("settings.mapDescription")}</Card.Description>
      </Card.Header>
      <Card.Content className="grid gap-4">
        <label className="flex items-center justify-between gap-4 rounded-lg border bg-background px-4 py-3">
          <div className="grid gap-1">
            <span className="text-sm font-medium">{t("map.requestFlows")}</span>
            <span className="text-sm text-muted-foreground">{t("settings.mapShowFlowsDescription")}</span>
          </div>
          <Switch isSelected={showFlows} onChange={setShowFlows} aria-label={t("map.requestFlows")} />
        </label>
        <label className="flex items-center justify-between gap-4 rounded-lg border bg-background px-4 py-3">
          <div className="grid gap-1">
            <span className="text-sm font-medium">{t("map.demandHeat")}</span>
            <span className="text-sm text-muted-foreground">{t("settings.mapShowHeatDescription")}</span>
          </div>
          <Switch isSelected={showHeat} onChange={setShowHeat} aria-label={t("map.demandHeat")} />
        </label>

        {canEditViewport ? (
          <div className="grid gap-4 rounded-lg border bg-background px-4 py-4">
            <div className="flex flex-wrap items-start justify-between gap-3">
              <div className="grid gap-1">
                <span className="text-sm font-medium">{t("settings.mapViewportTitle")}</span>
                <span className="text-sm text-muted-foreground">{t("settings.mapViewportDescription")}</span>
              </div>
              <Button
                variant="outline"
                size="sm"
                onPress={resetViewport}
                isDisabled={
                  viewport.visibleStartPx === DEFAULT_MAP_VIEWPORT.visibleStartPx
                  && viewport.visibleEndPx === DEFAULT_MAP_VIEWPORT.visibleEndPx
                  && viewport.verticalPanPx === DEFAULT_MAP_VIEWPORT.verticalPanPx
                }
              >
                <RotateCcw className="h-3.5 w-3.5" />
                {t("settings.mapViewportReset")}
              </Button>
            </div>
            <div className="grid gap-3 md:grid-cols-3">
              <ViewportField
                id="map-visible-start"
                label={t("settings.mapVisibleStartPx")}
                description={t("settings.mapVisibleStartPxDescription")}
                value={viewport.visibleStartPx}
                onChange={(value) => setViewport((prev) => ({ ...prev, visibleStartPx: value }))}
              />
              <ViewportField
                id="map-visible-end"
                label={t("settings.mapVisibleEndPx")}
                description={t("settings.mapVisibleEndPxDescription")}
                value={viewport.visibleEndPx}
                onChange={(value) => setViewport((prev) => ({ ...prev, visibleEndPx: value }))}
              />
              <ViewportField
                id="map-vertical-pan"
                label={t("settings.mapVerticalPanPx")}
                description={t("settings.mapVerticalPanPxDescription")}
                value={viewport.verticalPanPx}
                onChange={(value) => setViewport((prev) => ({ ...prev, verticalPanPx: value }))}
              />
            </div>
          </div>
        ) : null}
      </Card.Content>
    </Card>
  );
}

function ViewportField({
  id,
  label,
  description,
  value,
  onChange,
}: {
  id: string;
  label: string;
  description: string;
  value: number;
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
    onChange(parsed);
  }, [draft, onChange, value]);

  return (
    <label className="grid gap-2" htmlFor={id}>
      <span className="text-sm font-medium">{label}</span>
      <Input
        id={id}
        type="number"
        value={draft}
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
