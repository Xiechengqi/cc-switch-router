"use client";

// Hallmark · component: rule editor · modern-minimal · existing Router tokens
// Pre-emit critique: P4 H4 E4 S5 R5 V4. Keyboard actions complement dragging.
import * as React from "react";
import { Button } from "@heroui/react";
import { Trash2 } from "lucide-react";
import { SharePoolPicker, type PickerAction } from "./model-hub-share-picker";
import { modelRouteTargets, type DraftModelRoute } from "@/lib/model-routing";
import type { ModelRoutingApp, UserModelRoutingShare } from "@/lib/types";
import type { AppLocale, MessageKey } from "@/lib/i18n";

export function modelRouteEditorId(clientId: string) { return `model-route-${encodeURIComponent(clientId)}`; }
export function ModelHubRuleEditor({ kind, position = 1, route, app, appLabel, shares, disabled, addBlockedReason, duplicate = false, onAction, onTargets, onModel, onRemove, locale, t }: {
  /** Fixed by the slot that renders the row, never inferred from its value:
   * an exact row whose draft model is `*` must keep its input and delete button. */
  kind: "default" | "exact"; position?: number;
  route?: DraftModelRoute | null; app: ModelRoutingApp; appLabel: string; shares: UserModelRoutingShare[];
  disabled: boolean; addBlockedReason?: string; duplicate?: boolean; onAction: (action: PickerAction) => boolean;
  onTargets: (ids: string[]) => void; onModel?: (model: string) => void; onRemove?: () => void;
  locale: AppLocale; t: (key: MessageKey, params?: Record<string, string | number>) => string;
}) {
  const errorId = React.useId();
  const exact = kind === "exact" && !!route;
  const ruleLabel = kind === "default" ? t("modelHub.passthrough") : route?.requestedModel.trim() || t("modelHub.unnamedRule", { order: position });
  return <section id={modelRouteEditorId(route?.clientId || `default:${app}`)} tabIndex={-1}
    className="grid min-w-0 gap-3 py-3 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring"
    aria-label={`${appLabel} · ${ruleLabel}`}>
    {exact ? <div className="flex min-w-0 items-end gap-2">
      <label className="grid min-w-0 flex-1 gap-1.5 text-xs text-muted-foreground">
        {t("modelHub.model")}
        <input value={route.requestedModel} disabled={disabled} aria-required="true"
          aria-invalid={duplicate || !route.requestedModel.trim() || !route.targetShareId ? true : undefined} aria-describedby={errorId}
          onChange={event => onModel?.(Array.from(event.target.value).slice(0, 200).join(""))}
          className="h-11 min-w-0 rounded-md border border-input bg-card px-3 text-sm text-foreground hover:bg-secondary focus-visible:outline-2 focus-visible:outline-ring disabled:opacity-50"
          placeholder={t("modelHub.modelPlaceholder")} />
      </label>
      <Button isIconOnly size="sm" variant="ghost" className="h-11 w-11 text-danger" isDisabled={disabled}
        aria-label={t("modelHub.removeRoute")} onClick={onRemove}><Trash2 className="size-4" aria-hidden /></Button>
    </div> : null}
    <SharePoolPicker shares={shares} selected={modelRouteTargets(route)} app={app} routeKey={route?.clientId || `default:${app}`}
      disabled={disabled} addBlockedReason={addBlockedReason} onChange={onTargets} onAction={onAction} locale={locale} t={t} />
    <p id={errorId} className="text-xs text-danger" role="status">{duplicate ? t("modelHub.validationDuplicate") : exact && (!route.requestedModel.trim() || !route.targetShareId) ? t("modelHub.validationRequired") : ""}</p>
  </section>;
}
