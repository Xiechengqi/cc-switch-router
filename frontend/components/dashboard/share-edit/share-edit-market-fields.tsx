"use client";

import { Checkbox, Input, ListBox, Select, TextArea } from "@heroui/react";
import * as React from "react";
import { isOfficialRuntime, marketLabel, runtimeModelSummary, type TFn } from "@/components/dashboard/share-dashboard-utils";
import { ShareAppLogo } from "@/components/dashboard/share-app-logo";
import type { DashboardMarket, ShareAppRuntimes, ShareUpstreamProvider, ShareView } from "@/lib/types";
import { SHARE_APP_LABELS, type CoreShareApp } from "@/lib/share-app";
import {
  applyRecommendedMarketDefaults,
  type ShareEditDraft,
} from "./share-edit-draft";
import { FieldGroup, MarketEmailChip } from "./share-edit-shared";
import { forSaleOptionLabel, ShareEditSection } from "./share-edit-section";

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
  activeShareApps: CoreShareApp[];
  tokenMarkets: DashboardMarket[];
  marketSelectKey: number;
  pricingInvalid: boolean;
  onForSaleChange: (next: "Yes" | "No" | "Free") => void;
  onDraftChange: (updater: (current: ShareEditDraft) => ShareEditDraft) => void;
  onMarketPicked: (raw: string) => void;
};

export function ShareEditSaleAccessFields({
  t,
  draft,
  activeShareApps,
  tokenMarkets,
  marketSelectKey,
  pricingInvalid,
  onForSaleChange,
  onDraftChange,
  onMarketPicked,
}: ShareEditSaleAccessFieldsProps) {
  const { forSale, marketAccessMode, selectedMarketEmails, priceInputs } = draft;
  const sharedPriceApp = activeShareApps[0];

  const availableMarkets = React.useMemo(() => {
    const blocked = new Set(selectedMarketEmails);
    return tokenMarkets
      .filter((market) => market.email && !blocked.has(market.email.toLowerCase()))
      .sort((a, b) => marketLabel(a).localeCompare(marketLabel(b)));
  }, [selectedMarketEmails, tokenMarkets]);

  const removeMarketEmail = (email: string) => {
    onDraftChange((current) => ({
      ...current,
      selectedMarketEmails: current.selectedMarketEmails.filter((value) => value !== email),
    }));
  };

  return (
    <>
      <div className={forSale === "Yes" ? "grid gap-3 sm:grid-cols-2" : "grid gap-3"}>
        <FieldGroup label={t("dashboard.field.forSale")}>
          <Select
            selectedKey={forSale}
            onSelectionChange={(key) => onForSaleChange(String(key || "No") as "Yes" | "No" | "Free")}
          >
            <Select.Trigger>
              <Select.Value>{forSaleOptionLabel(forSale, t)}</Select.Value>
              <Select.Indicator />
            </Select.Trigger>
            <Select.Popover className="share-edit-popover light !bg-white !text-slate-900">
              <ListBox>
                {(["No", "Yes", "Free"] as const).map((item) => (
                  <ListBox.Item key={item} id={item}>
                    {forSaleOptionLabel(item, t)}
                  </ListBox.Item>
                ))}
              </ListBox>
            </Select.Popover>
          </Select>
        </FieldGroup>

        {forSale === "Yes" ? (
          <FieldGroup label={t("dashboard.field.marketAccess")}>
            <Select
              key={marketSelectKey}
              selectedKey={null}
              onSelectionChange={(key) => onMarketPicked(String(key || ""))}
            >
              <Select.Trigger>
                <Select.Value>
                  {marketAccessMode === "all" ? t("dashboard.allMarkets") : t("dashboard.addMarket")}
                </Select.Value>
                <Select.Indicator />
              </Select.Trigger>
              <Select.Popover className="share-edit-popover light !bg-white !text-slate-900">
                <ListBox>
                  <ListBox.Item id="__all__">{t("dashboard.allMarkets")}</ListBox.Item>
                  {availableMarkets.map((market) => (
                    <ListBox.Item key={market.email} id={market.email.toLowerCase()}>
                      {marketLabel(market)}
                      <span className="ml-1 text-muted-foreground">· {market.email}</span>
                    </ListBox.Item>
                  ))}
                </ListBox>
              </Select.Popover>
            </Select>
          </FieldGroup>
        ) : null}
      </div>

      {forSale === "Yes" && marketAccessMode === "selected" ? (
        <FieldGroup label={t("dashboard.field.selectedMarkets")} hint={t("dashboard.hint.selectedMarkets")}>
          {selectedMarketEmails.length ? (
            <div className="flex flex-wrap gap-1.5">
              {selectedMarketEmails.map((email) => {
                const meta = tokenMarkets.find((market) => (market.email || "").toLowerCase() === email);
                const label = meta ? marketLabel(meta) : email;
                return <MarketEmailChip key={email} label={label} onRemove={() => removeMarketEmail(email)} />;
              })}
            </div>
          ) : (
            <div className="rounded-lg border border-dashed border-border bg-muted/30 px-3 py-2 text-xs text-muted-foreground">
              {t("dashboard.noAuthorizedMarkets")}
            </div>
          )}
        </FieldGroup>
      ) : null}

      {forSale === "Yes" && marketAccessMode === "all" ? (
        <div className="rounded-lg border border-primary/20 bg-primary/5 px-3 py-2 text-xs text-primary">
          {t("dashboard.allMarketsSelected")}
          <button
            type="button"
            className="ml-3 text-[11px] underline decoration-dotted underline-offset-2 hover:text-primary/80"
            onClick={() =>
              onDraftChange((current) =>
                applyRecommendedMarketDefaults(
                  { ...current, marketAccessMode: "selected", selectedMarketEmails: [] },
                  tokenMarkets,
                ),
              )
            }
          >
            {t("dashboard.switchToSelected")}
          </button>
        </div>
      ) : null}

      {forSale === "Yes" ? (
        <div className="grid gap-1.5 text-sm">
          <div className="flex flex-wrap items-baseline gap-x-2 gap-y-1">
            <span className="mono-label text-muted-foreground">{t("dashboard.field.modelPricing")}</span>
            <span className="text-xs text-muted-foreground">{t("dashboard.hint.modelPricing")}</span>
          </div>
          <div className="grid max-w-sm gap-1">
            <span className="mono-label text-muted-foreground">
              {activeShareApps.map((app) => SHARE_APP_LABELS[app]).join(" / ")}
            </span>
            <Input
              type="number"
              min={1}
              max={100}
              step={1}
              value={sharedPriceApp ? priceInputs[sharedPriceApp] : ""}
              disabled={!sharedPriceApp}
              placeholder={sharedPriceApp ? t("common.unset") : t("dashboard.noCurrentNode")}
              onChange={(event) => {
                const value = event.target.value;
                onDraftChange((current) => ({
                  ...current,
                  priceInputs: {
                    ...current.priceInputs,
                    ...Object.fromEntries(activeShareApps.map((app) => [app, value])),
                  },
                }));
              }}
            />
          </div>
          {pricingInvalid ? <span className="text-xs text-red-600">{t("dashboard.fieldInvalid")}</span> : null}
        </div>
      ) : null}
    </>
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
