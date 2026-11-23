"use client";

import {
  ShareProviderLogo,
} from "@/components/dashboard/share-provider-logo";
import { ShareAppLogo } from "@/components/dashboard/share-app-logo";
import { SHARE_APP_LABELS } from "@/lib/share-app";
import { isCoreShareApp } from "@/components/dashboard/share-market/market-utils";
import { shareAccountPresentation } from "../share-account-presentation";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { marketCapabilityRuntime, primaryMarketCapability } from "./market-utils";
import type { AppLocale } from "@/lib/i18n";
import type { PublicProviderIdentity } from "@/lib/types";

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
