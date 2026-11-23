"use client";

import {
  ShareProviderLogo,
} from "@/components/dashboard/share-provider-logo";
import { ShareAppLogo } from "@/components/dashboard/share-app-logo";
import { SHARE_APP_LABELS, CORE_SHARE_APPS } from "@/lib/share-app";
import { isCoreShareApp } from "@/components/dashboard/share-market/market-utils";
import { shareAccountPresentation } from "../share-account-presentation";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { formatShareStatus } from "../share-dashboard-utils";
import { capabilityModelLabel, marketCapabilityRuntime, primaryMarketCapability } from "./market-utils";
import { cn } from "@/lib/utils";
import type { AppLocale, MessageKey } from "@/lib/i18n";
import type { PublicProviderIdentity, ShareMarketOwnedShare } from "@/lib/types";

type ShareIdentityProvider = Pick<Parameters<typeof marketCapabilityRuntime>[0],
  "app" | "providerName" | "providerType" | "kind" | "subscriptionLevel" | "quota">;

type ShareIdentitySource = {
  appType?: string;
  providerIdentity?: PublicProviderIdentity;
  subdomain?: string;
  shareName?: string;
  supportedApps?: string[];
  apps?: string[];
  appCapabilities?: ShareIdentityProvider[];
};

function providerStatusPrimaryLine(source: ShareIdentitySource, locale: AppLocale, details = false) {
  const enabledApps = source.supportedApps || source.apps || [];
  const capability = primaryMarketCapability((source.appCapabilities || []).filter(
    item => !enabledApps.length || enabledApps.includes(item.app),
  ), source.appType);
  if (!capability) return "";
  const runtime = marketCapabilityRuntime(capability);
  const view = shareAccountPresentation(runtime, locale, {
    publicApiLabel: capability.providerName || capability.providerType,
  });
  const line = details ? view.primaryTitle : view.primaryLine;
  return line && line !== "-" ? line : "";
}

export function MarketProviderLogos({
  source,
  size = 16,
}: {
  source: ShareIdentitySource;
  size?: number;
}) {
  return (
    <ShareProviderLogo provider={{ providerIdentity: source.providerIdentity }} size={size} />
  );
}

export function MarketShareApps({
  apps,
  size = 14,
}: {
  apps?: string[];
  size?: number;
}) {
  const coreApps = (apps || []).filter(isCoreShareApp);
  if (!coreApps.length) return null;
  return (
    <span className="inline-flex shrink-0 items-center gap-1" title={coreApps.map((app) => SHARE_APP_LABELS[app]).join(" / ")}>
      {coreApps.map((app) => <ShareAppLogo key={app} app={app} size={size} />)}
    </span>
  );
}

const OWNED_SHARE_STATUS_KEYS: Record<string, MessageKey> = {
  active: "shareMarket.shareStatus.active",
  paused: "shareMarket.shareStatus.paused",
  expired: "shareMarket.shareStatus.expired",
};

const OWNED_SHARE_STATUS_DOT_CLASS: Record<string, string> = {
  active: "bg-emerald-500",
  paused: "bg-amber-500",
  expired: "bg-slate-400",
};

/** Model lines for the owned-share picker: one "icon + model" entry per supported core app. */
export function ownedShareModelEntries(
  share: Pick<ShareMarketOwnedShare, "appCapabilities" | "supportedApps">,
  passthroughLabel: string,
  unknownLabel: string,
) {
  const supportedApps = share.supportedApps || [];
  const capabilities = share.appCapabilities || [];
  return CORE_SHARE_APPS.filter((app) => supportedApps.includes(app)).flatMap((app) => {
    const capability = capabilities.find((item) => item.app === app);
    if (!capability) return [];
    return [{ app, model: capabilityModelLabel(capability, passthroughLabel, unknownLabel) }];
  });
}

/** Owned-share option in the add-listing picker: provider + subdomain + status, then per-app models. */
export function OwnedShareOptionIdentity({
  share,
  size = 16,
}: {
  share: ShareMarketOwnedShare;
  size?: number;
}) {
  const { t } = useLocaleText();
  const subdomain = share.subdomain?.trim() || share.shareName || "";
  const status = String(share.shareStatus || "").trim().toLowerCase();
  const statusKey = OWNED_SHARE_STATUS_KEYS[status];
  const statusLabel = statusKey ? t(statusKey) : formatShareStatus(share.shareStatus);
  const entries = ownedShareModelEntries(
    share,
    t("shareMarket.modelPassthrough"),
    t("shareMarket.catalog.modelUnknown"),
  );
  return (
    <span className="grid min-w-0 gap-0.5">
      <span className="inline-flex min-w-0 items-center gap-1.5">
        <MarketProviderLogos source={share} size={size} />
        {subdomain ? (
          <strong className="min-w-0 truncate font-mono text-xs font-semibold text-slate-800" title={subdomain}>
            {subdomain}
          </strong>
        ) : null}
        <span
          className="inline-flex shrink-0 items-center gap-1 text-[11px] text-slate-500"
          title={`${t("shareMarket.status")}: ${statusLabel}`}
        >
          <span
            className={cn("h-1.5 w-1.5 rounded-full", OWNED_SHARE_STATUS_DOT_CLASS[status] ?? "bg-slate-400")}
          />
          {statusLabel}
        </span>
      </span>
      {entries.length ? (
        <span className="flex min-w-0 items-center gap-x-2">
          {entries.map((entry) => (
            <span key={entry.app} className="inline-flex min-w-0 items-center gap-1">
              <ShareAppLogo app={entry.app} size={Math.max(12, size - 4)} />
              <span
                className="truncate text-[11px] font-normal text-slate-400"
                title={`${SHARE_APP_LABELS[entry.app]}: ${entry.model}`}
              >
                {entry.model}
              </span>
            </span>
          ))}
        </span>
      ) : null}
    </span>
  );
}

export function MarketShareIdentity({
  source,
  size = 16,
  showStatusLine = false,
}: {
  source: ShareIdentitySource;
  size?: number;
  showStatusLine?: boolean;
}) {
  const { locale } = useLocaleText();
  const subdomain = source.subdomain?.trim() || source.shareName || "";
  const apps = source.supportedApps || source.apps;
  const statusLine = showStatusLine ? providerStatusPrimaryLine(source, locale) : "";
  return (
    <span className="grid min-w-0 gap-0.5">
      <span className="inline-flex min-w-0 items-center gap-1.5">
        <MarketProviderLogos source={source} size={size} />
        {subdomain ? (
          <strong className="min-w-0 truncate font-mono text-xs font-semibold text-slate-800" title={subdomain}>
            {subdomain}
          </strong>
        ) : null}
        <MarketShareApps apps={apps} size={Math.max(12, size - 2)} />
      </span>
      {statusLine ? (
        <span className="min-w-0 truncate text-[11px] font-normal text-slate-400" title={providerStatusPrimaryLine(source, locale, true)}>
          {statusLine}
        </span>
      ) : null}
    </span>
  );
}
