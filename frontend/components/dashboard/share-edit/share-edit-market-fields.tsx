"use client";

import { Checkbox, Input } from "@heroui/react";
import * as React from "react";
import { isOfficialRuntime, runtimeModelSummary, type TFn } from "@/components/dashboard/share-dashboard-utils";
import { ShareAppLogo } from "@/components/dashboard/share-app-logo";
import type { ShareAppRuntimes, ShareUpstreamProvider, ShareView } from "@/lib/types";
import { SHARE_APP_LABELS, type CoreShareApp } from "@/lib/share-app";
import {
  type ShareEditDraft,
} from "./share-edit-draft";
import { FieldGroup } from "./share-edit-shared";
import { ShareEditSection } from "./share-edit-section";

function looksLikeEmail(value: string) {
  return /^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(value.trim());
}

function providerHint(runtime?: ShareUpstreamProvider) {
  if (!runtime) return "";
  if (isOfficialRuntime(runtime)) return "Official";
  if (runtime.providerName?.trim()) return "";
  const hint = String(runtime.apiUrl || runtime.kind || "").trim();
  if (!hint || looksLikeEmail(hint)) return "";
  return hint;
}

function providerTitle(runtime?: ShareUpstreamProvider) {
  const title = String(runtime?.providerName || runtime?.kind || runtime?.providerType || "").trim();
  if (!title || looksLikeEmail(title)) return "";
  return title;
}

export type ShareEditSaleAccessFieldsProps = {
  t: TFn;
  draft: ShareEditDraft;
  disabled?: boolean;
  onDraftChange: (updater: (current: ShareEditDraft) => ShareEditDraft) => void;
};

export function ShareEditSaleAccessFields({
  t,
  draft,
  disabled,
  onDraftChange,
}: ShareEditSaleAccessFieldsProps) {
  return (
    <FieldGroup
      label={t("dashboard.field.freeAccess")}
      hint={t("dashboard.hint.freeAccess")}
    >
      <Checkbox
        isSelected={draft.freeAccess}
        isDisabled={disabled}
        onChange={(freeAccess: boolean) =>
          onDraftChange((current) => ({
            ...current,
            freeAccess,
          }))
        }
      >
        <Checkbox.Control>
          <Checkbox.Indicator />
        </Checkbox.Control>
        <Checkbox.Content>
          <span className="text-sm">{t("dashboard.freeAccessLabel")}</span>
        </Checkbox.Content>
      </Checkbox>
      {draft.freeAccess ? (
        <p className="text-xs text-amber-700">{t("dashboard.hint.freeAccessUserOverrides")}</p>
      ) : null}
    </FieldGroup>
  );
}

export type ShareEditMarketFieldsProps = {
  t: TFn;
  share: ShareView;
  activeShareApps: CoreShareApp[];
  draft: ShareEditDraft;
  descriptionInvalid: boolean;
  appApiInvalid: boolean;
  disabled?: boolean;
  onDescriptionChange: (value: string) => void;
  onDraftChange: (updater: (current: ShareEditDraft) => ShareEditDraft) => void;
};

export function ShareEditMarketFields({
  t,
  share,
  activeShareApps,
  draft,
  descriptionInvalid,
  appApiInvalid,
  disabled,
  onDescriptionChange,
  onDraftChange,
}: ShareEditMarketFieldsProps) {
  return (
    <>
      <div className="grid gap-1">
        <Input
          value={draft.description}
          maxLength={200}
          placeholder={t("dashboard.shareEdit.descriptionPlaceholder")}
          disabled={disabled}
          aria-invalid={descriptionInvalid}
          aria-label={t("dashboard.field.description")}
          onChange={(event) => onDescriptionChange(event.target.value)}
        />
        {descriptionInvalid ? (
          <span className="text-xs text-red-600">{t("dashboard.hint.maxChars")}</span>
        ) : null}
      </div>

      <ShareEditSection title={t("dashboard.shareEdit.section.market")}>
        <div className="grid grid-cols-3 gap-2">
          {activeShareApps.map((app) => {
            const runtime = share.appRuntimes?.[app as keyof ShareAppRuntimes];
            const title = providerTitle(runtime);
            const hint = providerHint(runtime);
            const models = runtimeModelSummary(runtime, t("dashboard.shareEdit.passthrough"));
            const enabled = Boolean(draft.enabledApps[app]);
            const lastEnabled =
              enabled && !activeShareApps.some((other) => other !== app && draft.enabledApps[other]);
            return (
              <Checkbox
                key={app}
                className={`w-full items-start rounded-xl border-0 px-3 py-2.5 shadow-none ${
                  enabled ? "bg-emerald-50 text-slate-900" : "bg-slate-50 text-slate-500"
                }`}
                isSelected={enabled}
                isDisabled={disabled || lastEnabled}
                aria-label={t("dashboard.shareEdit.appApiToggle", { app: SHARE_APP_LABELS[app] })}
                onChange={(value: boolean) => {
                  onDraftChange((current) => {
                    if (!value && !activeShareApps.some((other) => other !== app && current.enabledApps[other])) {
                      return current;
                    }
                    return {
                      ...current,
                      enabledApps: { ...current.enabledApps, [app]: value },
                    };
                  });
                }}
              >
                <Checkbox.Control className="mt-0.5">
                  <Checkbox.Indicator />
                </Checkbox.Control>
                <Checkbox.Content className="min-w-0 flex-1">
                  <div className="flex min-w-0 items-start gap-2">
                    <ShareAppLogo app={app} size={16} className={enabled ? undefined : "opacity-60"} />
                    <div className="min-w-0 flex-1">
                      <div className="flex min-w-0 flex-col gap-0.5">
                        <span className={`text-sm font-medium ${enabled ? "text-slate-900" : "text-slate-500"}`}>
                          {SHARE_APP_LABELS[app]} API
                        </span>
                        {title ? <span className="truncate text-xs text-slate-500">{title}</span> : null}
                      </div>
                      <div className={`mt-0.5 whitespace-normal break-all text-[11px] ${enabled ? "text-slate-500" : "text-slate-400"}`}>
                        {[hint, models].filter(Boolean).join(" · ") || t("dashboard.noCurrentNode")}
                      </div>
                    </div>
                  </div>
                </Checkbox.Content>
              </Checkbox>
            );
          })}
          {appApiInvalid ? (
            <span className="col-span-3 text-xs text-red-600">{t("dashboard.shareEdit.appApiRequired")}</span>
          ) : null}
        </div>
      </ShareEditSection>
    </>
  );
}
