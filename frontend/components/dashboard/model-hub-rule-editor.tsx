"use client";

// Hallmark · component: rule editor · modern-minimal · existing Router tokens
// Pre-emit critique: P4 H4 E4 S5 R5 V4. Keyboard actions complement dragging.
import * as React from "react";
import { Trash2 } from "lucide-react";
import { SharePoolPicker, type PickerAction } from "./model-hub-share-picker";
import { modelRouteTargets, type DraftModelRoute } from "@/lib/model-routing";
import type { ModelRoutingApp, UserModelRoutingShare } from "@/lib/types";
import type { AppLocale, MessageKey } from "@/lib/i18n";
import type { ShareRouteTraffic } from "@/lib/model-routing-traffic";

export function modelRouteEditorId(clientId: string) { return `model-route-${encodeURIComponent(clientId)}`; }
export function ModelHubRuleEditor({ kind, position = 1, route, app, appLabel, shares, disabled, addBlockedReason, duplicate = false, traffic, onAction, onTargets, onModel, onRemove, locale, t }: {
  /** Fixed by the slot that renders the row, never inferred from its value:
   * an exact row whose draft model is `*` must keep its input and delete button. */
  kind: "default" | "exact"; position?: number;
  route?: DraftModelRoute | null; app: ModelRoutingApp; appLabel: string; shares: UserModelRoutingShare[];
  disabled: boolean; addBlockedReason?: string; duplicate?: boolean; traffic?: Map<string, ShareRouteTraffic>;
  onAction: (action: PickerAction) => boolean;
  onTargets: (ids: string[]) => void; onModel?: (model: string) => void; onRemove?: () => void;
  locale: AppLocale; t: (key: MessageKey, params?: Record<string, string | number>) => string;
}) {
  const errorId = React.useId();
  const exact = kind === "exact" && !!route;
  const ruleLabel = kind === "default" ? t("modelHub.passthrough") : route?.requestedModel.trim() || t("modelHub.unnamedRule", { order: position });
  return <section id={modelRouteEditorId(route?.clientId || `default:${app}`)} tabIndex={-1}
    className="grid min-w-0 gap-3 rounded-lg focus-visible:outline-2 focus-visible:outline-offset-4 focus-visible:outline-ring"
    aria-label={`${appLabel} · ${ruleLabel}`}>
    {exact ? <div className="flex min-w-0 items-end gap-2">
      <label className="grid min-w-0 flex-1 gap-1.5 font-mono text-[11px] uppercase tracking-[0.12em] text-muted-foreground">
        {t("modelHub.model")}
        <input value={route.requestedModel} disabled={disabled} aria-required="true"
          aria-invalid={duplicate || !route.requestedModel.trim() || !route.targetShareId ? true : undefined} aria-describedby={errorId}
          onChange={event => onModel?.(Array.from(event.target.value).slice(0, 200).join(""))}
          className="h-11 min-w-0 rounded-lg border border-input bg-card px-3 font-mono text-sm normal-case tracking-normal text-foreground transition-colors duration-200 placeholder:text-muted-foreground/50 hover:border-primary/30 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring aria-[invalid=true]:border-red-300 disabled:opacity-50"
          placeholder={t("modelHub.modelPlaceholder")} />
      </label>
      {/* Native control, like the Transfer list: @heroui/react is ESM-only and
          would stop the server-rendered editor tests from loading under tsx. */}
      <button type="button" disabled={disabled} aria-label={t("modelHub.removeRoute")} onClick={onRemove}
        className="inline-flex h-11 w-11 shrink-0 items-center justify-center rounded-lg text-danger transition-colors duration-200 hover:bg-red-50 active:bg-red-50 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring disabled:cursor-not-allowed disabled:opacity-40">
        <Trash2 className="size-4" aria-hidden />
      </button>
    </div> : null}
    <SharePoolPicker shares={shares} selected={modelRouteTargets(route)} app={app} routeKey={route?.clientId || `default:${app}`}
      disabled={disabled} addBlockedReason={addBlockedReason} traffic={traffic} onChange={onTargets} onAction={onAction} locale={locale} t={t} />
    <p id={errorId} className="text-xs text-danger empty:hidden" role="status">{duplicate ? t("modelHub.validationDuplicate") : exact && (!route.requestedModel.trim() || !route.targetShareId) ? t("modelHub.validationRequired") : ""}</p>
  </section>;
}
