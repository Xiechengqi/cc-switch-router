"use client";

import {
  resolveShareProviderLogo,
  ShareProviderLogo,
} from "@/components/dashboard/share-provider-logo";
import { ShareAppLogo } from "@/components/dashboard/share-app-logo";
import { SHARE_APP_LABELS } from "@/lib/share-app";
import type { ShareMarketAppCapability } from "@/lib/types";
import { isCoreShareApp } from "@/components/dashboard/share-market/market-utils";

type ShareIdentitySource = {
  subdomain?: string;
  shareName?: string;
  supportedApps?: string[];
  appCapabilities?: ShareMarketAppCapability[];
};

function uniqueProviderCapabilities(source: ShareIdentitySource) {
  const enabledApps = new Set(source.supportedApps || []);
  const capabilities = (source.appCapabilities || []).filter(
    (item) => isCoreShareApp(item.app) && (!enabledApps.size || enabledApps.has(item.app)),
  );
  return capabilities.reduce<ShareMarketAppCapability[]>((result, capability) => {
    const logo = resolveShareProviderLogo(capability);
    const key = logo?.key || capability.providerType || capability.providerName || capability.app;
    if (!result.some((item) => {
      const itemLogo = resolveShareProviderLogo(item);
      return (itemLogo?.key || item.providerType || item.providerName || item.app) === key;
    })) result.push(capability);
    return result;
  }, []);
}

export function MarketProviderLogos({
  source,
  size = 16,
}: {
  source: ShareIdentitySource;
  size?: number;
}) {
  const entries = uniqueProviderCapabilities(source);
  if (!entries.length) return null;
  return (
    <span className="inline-flex shrink-0 items-center gap-1">
      {entries.slice(0, 3).map((capability) => (
        <ShareProviderLogo
          key={`${capability.app}:${capability.providerType || capability.providerName || "provider"}`}
          provider={capability}
          fallbackApp={capability.app as "claude" | "codex" | "gemini"}
          size={size}
        />
      ))}
    </span>
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
}: {
  source: ShareIdentitySource;
  size?: number;
}) {
  const subdomain = source.subdomain?.trim() || source.shareName || "";
  return (
    <span className="inline-flex min-w-0 items-center gap-1.5">
      <MarketProviderLogos source={source} size={size} />
      {subdomain ? (
        <strong className="min-w-0 truncate font-mono text-xs font-semibold text-slate-800" title={subdomain}>
          {subdomain}
        </strong>
      ) : null}
      <MarketShareApps apps={source.supportedApps} size={Math.max(12, size - 2)} />
    </span>
  );
}
