"use client";

import { Checkbox, Input, ListBox, Select, TextArea } from "@heroui/react";
import * as React from "react";
import { isOfficialRuntime, marketLabel, type TFn } from "@/components/dashboard/share-dashboard-utils";
import type { DashboardMarket, ShareAppRuntimes, ShareUpstreamProvider, ShareView } from "@/lib/types";
import type { CoreShareApp } from "@/lib/share-app";
import {
  applyRecommendedMarketDefaults,
  PRICE_APPS,
  recommendedShareMarketEmail,
  type ShareEditDraft,
} from "./share-edit-draft";
import { FieldGroup, MarketEmailChip } from "./share-edit-shared";
import { forSaleOptionLabel, ShareEditSection } from "./share-edit-section";

function providerHint(runtime?: ShareUpstreamProvider) {
  if (!runtime) return "";
  if (isOfficialRuntime(runtime)) return "Official";
  return runtime.accountEmail || runtime.apiUrl || runtime.kind || "";
}

export type ShareEditMarketFieldsProps = {
  t: TFn;
  share: ShareView;
  shareApp: CoreShareApp;
  draft: ShareEditDraft;
  tokenMarkets: DashboardMarket[];
  shareMarkets: DashboardMarket[];
  marketSelectKey: number;
  descriptionLength: number;
  descriptionInvalid: boolean;
  pricingInvalid: boolean;
  shareMarketInvalid: boolean;
  onDescriptionChange: (value: string) => void;
  onForSaleChange: (next: "Yes" | "No" | "Free") => void;
  onDraftChange: (updater: (current: ShareEditDraft) => ShareEditDraft) => void;
  onMarketPicked: (raw: string) => void;
};

export function ShareEditMarketFields({
  t,
  share,
  shareApp,
  draft,
  tokenMarkets,
  shareMarkets,
  marketSelectKey,
  descriptionLength,
  descriptionInvalid,
  pricingInvalid,
  shareMarketInvalid,
  onDescriptionChange,
  onForSaleChange,
  onDraftChange,
  onMarketPicked,
}: ShareEditMarketFieldsProps) {
  const { forSale, saleMarketKind, marketAccessMode, selectedMarketEmails, selectedShareMarketEmail, priceInputs } =
    draft;

  const availableMarkets = React.useMemo(() => {
    if (saleMarketKind === "share") {
      return [...shareMarkets].sort((a, b) => marketLabel(a).localeCompare(marketLabel(b)));
    }
    const blocked = new Set(selectedMarketEmails);
    return tokenMarkets
      .filter((market) => market.email && !blocked.has(market.email.toLowerCase()))
      .sort((a, b) => marketLabel(a).localeCompare(marketLabel(b)));
  }, [saleMarketKind, selectedMarketEmails, shareMarkets, tokenMarkets]);

  const handleSaleMarketKindChange = (next: "token" | "share") => {
    onDraftChange((current) => {
      let nextDraft: ShareEditDraft = {
        ...current,
        saleMarketKind: next,
        marketAccessMode: next === "share" ? "selected" : current.marketAccessMode,
        selectedMarketEmails: next === "share" ? [] : current.selectedMarketEmails,
        selectedShareMarketEmail: next === "token" ? "" : current.selectedShareMarketEmail,
        priceInputs: next === "share" ? { claude: "", codex: "", gemini: "" } : current.priceInputs,
      };
      if (next === "share" && !nextDraft.selectedShareMarketEmail) {
        nextDraft.selectedShareMarketEmail = recommendedShareMarketEmail(shareMarkets);
      }
      return applyRecommendedMarketDefaults(nextDraft, tokenMarkets, shareMarkets);
    });
  };

  const removeMarketEmail = (email: string) => {
    onDraftChange((current) => ({
      ...current,
      selectedMarketEmails: current.selectedMarketEmails.filter((value) => value !== email),
    }));
  };

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
        <div className="grid gap-3 sm:grid-cols-3">
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
            <FieldGroup label={t("dashboard.field.marketType")}>
              <Select
                selectedKey={saleMarketKind}
                onSelectionChange={(key) => handleSaleMarketKindChange(String(key || "token") as "token" | "share")}
              >
                <Select.Trigger>
                  <Select.Value>
                    {saleMarketKind === "share" ? t("dashboard.shareMarket") : t("dashboard.tokenMarket")}
                  </Select.Value>
                  <Select.Indicator />
                </Select.Trigger>
                <Select.Popover className="share-edit-popover light !bg-white !text-slate-900">
                  <ListBox>
                    <ListBox.Item id="token">{t("dashboard.tokenMarket")}</ListBox.Item>
                    <ListBox.Item id="share">{t("dashboard.shareMarket")}</ListBox.Item>
                  </ListBox>
                </Select.Popover>
              </Select>
            </FieldGroup>
          ) : null}

          <FieldGroup
            label={t("dashboard.field.marketAccess")}
            hint={
              forSale !== "Yes"
                ? t("dashboard.hint.forSaleOnly")
                : saleMarketKind === "share"
                  ? t("dashboard.hint.shareMarketSingle")
                  : undefined
            }
            invalid={shareMarketInvalid}
          >
            <Select
              key={marketSelectKey}
              selectedKey={null}
              onSelectionChange={(key) => onMarketPicked(String(key || ""))}
              isDisabled={forSale !== "Yes" || (saleMarketKind === "share" && shareMarkets.length === 0)}
            >
              <Select.Trigger>
                <Select.Value>
                  {saleMarketKind === "share"
                    ? selectedShareMarketEmail
                      ? marketLabel(
                          shareMarkets.find((market) => market.email.toLowerCase() === selectedShareMarketEmail) || {
                            email: selectedShareMarketEmail,
                            publicBaseUrl: "",
                            subdomain: "",
                          },
                        )
                      : t("dashboard.selectShareMarket")
                    : marketAccessMode === "all"
                      ? t("dashboard.allMarkets")
                      : t("dashboard.addMarket")}
                </Select.Value>
                <Select.Indicator />
              </Select.Trigger>
              <Select.Popover className="share-edit-popover light !bg-white !text-slate-900">
                <ListBox>
                  {saleMarketKind === "token" ? (
                    <ListBox.Item id="__all__">{t("dashboard.allMarkets")}</ListBox.Item>
                  ) : null}
                  {availableMarkets.map((market) => (
                    <ListBox.Item key={market.email} id={market.email.toLowerCase()}>
                      {marketLabel(market)}
                      <span className="ml-1 text-muted-foreground">· {market.email}</span>
                    </ListBox.Item>
                  ))}
                </ListBox>
              </Select.Popover>
            </Select>
            {shareMarketInvalid ? <span className="text-xs text-red-600">{t("dashboard.fieldInvalid")}</span> : null}
          </FieldGroup>
        </div>

        {forSale === "Yes" && saleMarketKind === "token" ? (
          <div className="grid gap-1.5 text-sm">
            <div className="flex flex-wrap items-baseline gap-x-2 gap-y-1">
              <span className="mono-label text-muted-foreground">{t("dashboard.field.modelPricing")}</span>
              <span className="text-xs text-muted-foreground">{t("dashboard.hint.modelPricing")}</span>
            </div>
            <div className="grid gap-3 sm:grid-cols-3">
              {PRICE_APPS.filter((app) => app.key === shareApp).map((app) => {
                const supported = !!share?.support?.[app.key];
                const hint = providerHint(share?.appRuntimes?.[app.key as keyof ShareAppRuntimes]);
                return (
                  <div key={app.key} className="grid gap-1">
                    <span className="mono-label text-muted-foreground">{app.label}</span>
                    <Input
                      type="number"
                      min={1}
                      max={100}
                      step={1}
                      value={priceInputs[app.key]}
                      disabled={!supported}
                      placeholder={supported ? t("common.unset") : t("dashboard.noCurrentNode")}
                      onChange={(event) =>
                        onDraftChange((current) => ({
                          ...current,
                          priceInputs: { ...current.priceInputs, [app.key]: event.target.value },
                        }))
                      }
                    />
                    <span className="truncate text-[11px] text-muted-foreground">{hint || "-"}</span>
                  </div>
                );
              })}
            </div>
            {pricingInvalid ? <span className="text-xs text-red-600">{t("dashboard.fieldInvalid")}</span> : null}
          </div>
        ) : null}

        {forSale === "Yes" && saleMarketKind === "token" && marketAccessMode === "selected" ? (
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

        {forSale === "Yes" && saleMarketKind === "token" && marketAccessMode === "all" ? (
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
                    shareMarkets,
                  ),
                )
              }
            >
              {t("dashboard.switchToSelected")}
            </button>
          </div>
        ) : null}
      </ShareEditSection>
    </>
  );
}
