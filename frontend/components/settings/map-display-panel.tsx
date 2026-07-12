"use client";

import { Card, Switch } from "@heroui/react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { useMapDisplaySettings } from "@/lib/map-display-settings";

export function MapDisplayPanel() {
  const { t } = useLocaleText();
  const { showFlows, setShowFlows, showHeat, setShowHeat } = useMapDisplaySettings();

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
      </Card.Content>
    </Card>
  );
}
