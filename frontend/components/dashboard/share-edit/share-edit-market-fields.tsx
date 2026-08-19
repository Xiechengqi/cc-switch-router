"use client";

import { Checkbox, TextArea } from "@heroui/react";
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

function providerHint(runtime?: ShareUpstreamProvider) {
  if (!runtime) return "";
  if (isOfficialRuntime(runtime)) return "Official";
  return runtime.accountEmail || runtime.apiUrl || runtime.kind || "";
}

function providerTitle(runtime?: ShareUpstreamProvider) {
  return runtime?.providerName || runtime?.kind || runtime?.providerType || "";
}

export type ShareEditSaleAccessFieldsProps = {
  t: TFn;
  draft: ShareEditDraft;
  onDraftChange: (updater: (current: ShareEditDraft) => ShareEditDraft) => void;
};

export function ShareEditSaleAccessFields({
  t,
  draft,
  onDraftChange,
}: ShareEditSaleAccessFieldsProps) {
  return (
    <FieldGroup
      label={t("dashboard.field.freeAccess")}
      hint={t("dashboard.hint.freeAccess")}
    >
      <Checkbox
        isSelected={draft.freeAccess}
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
  descriptionLength: number;
  descriptionInvalid: boolean;
  appApiInvalid: boolean;
  onDescriptionChange: (value: string) => void;
  onDraftChange: (updater: (current: ShareEditDraft) => ShareEditDraft) => void;
};

export function ShareEditMarketFields({
  t,
  share,
  activeShareApps,
  draft,
  descriptionLength,
  descriptionInvalid,
  appApiInvalid,
  onDescriptionChange,
  onDraftChange,
}: ShareEditMarketFieldsProps) {
  return (
    <>
      <ShareEditSection title={t("dashboard.shareEdit.section.overview")}>
        <FieldGroup
          label={t("dashboard.field.description")}
          hint={
            <span>
              {t("dashboard.hint.maxChars")}
              <span className="ml-2 font-mono">{descriptionLength}/200</span>
            </span>
          }
          invalid={descriptionInvalid}
        >
          <TextArea
            value={draft.description}
            maxLength={200}
            onChange={(event) => onDescriptionChange(event.target.value)}
          />
        </FieldGroup>
      </ShareEditSection>

      <ShareEditSection title={t("dashboard.shareEdit.section.market")}>
        <div className="grid gap-2">
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
                className="w-full items-start rounded-xl border border-slate-200 bg-slate-50/70 px-3 py-2.5"
                isSelected={enabled}
                isDisabled={lastEnabled}
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
                  <div className="flex min-w-0 items-start gap-3">
                    <ShareAppLogo app={app} size={16} />
                    <div className="min-w-0 flex-1">
                      <div className="flex flex-wrap items-center gap-x-2 gap-y-0.5">
                        <span className="text-sm font-medium text-slate-900">{SHARE_APP_LABELS[app]} API</span>
                        {title ? <span className="truncate text-xs text-slate-500">{title}</span> : null}
                      </div>
                      <div className="mt-0.5 truncate text-[11px] text-slate-500">
                        {[hint, models].filter(Boolean).join(" · ") || t("dashboard.noCurrentNode")}
                      </div>
                    </div>
                  </div>
                </Checkbox.Content>
              </Checkbox>
            );
          })}
          {appApiInvalid ? (
            <span className="text-xs text-red-600">{t("dashboard.shareEdit.appApiRequired")}</span>
          ) : null}
        </div>
      </ShareEditSection>
    </>
  );
}
