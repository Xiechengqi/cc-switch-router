"use client";

import {
  resolveShareProviderLogo,
  ShareProviderLogo,
} from "@/components/dashboard/share-provider-logo";
import { ShareAppLogo } from "@/components/dashboard/share-app-logo";
import { CORE_SHARE_APPS, SHARE_APP_LABELS } from "@/lib/share-app";
import { isCoreShareApp } from "@/components/dashboard/share-market/market-utils";
import {
  isApiProviderRuntime,
  providerApiEndpoint,
  providerQuotaStatusLine,
} from "@/components/dashboard/share-dashboard-utils";
import { useLocaleText } from "@/components/i18n/locale-provider";
import type { AppLocale } from "@/lib/i18n";
import type { ShareUpstreamProvider } from "@/lib/types";

type ShareIdentityProvider = {
  app: string;
  providerName?: string;
  providerType?: string;
  kind?: string;
  apiUrl?: string;
  subscriptionLevel?: string;
  quota?: ShareUpstreamProvider["quota"];
};

type ShareIdentitySource = {
  subdomain?: string;
  shareName?: string;
  supportedApps?: string[];
  apps?: string[];
  appCapabilities?: ShareIdentityProvider[];
};

function capabilityRuntime(capability: ShareIdentityProvider): ShareUpstreamProvider {
  return {
    app: capability.app,
    kind: capability.kind || capability.providerType,
    providerType: capability.providerType || capability.kind,
    providerName: capability.providerName,
    apiUrl: capability.apiUrl,
    subscriptionLevel: capability.subscriptionLevel,
    quota: capability.quota,
  };
}

function providerStatusPrimaryLine(source: ShareIdentitySource, locale: AppLocale) {
  const enabledApps = source.supportedApps || source.apps || [];
  const preferredApp = CORE_SHARE_APPS.find((app) => enabledApps.includes(app)) || enabledApps[0];
  const capability =
    (source.appCapabilities || []).find((item) => item.app === preferredApp)
    || uniqueProviderCapabilities(source)[0];
  if (!capability) return "";
  const runtime = capabilityRuntime(capability);
  const line = isApiProviderRuntime(runtime)
    ? providerApiEndpoint(runtime)
    : providerQuotaStatusLine(runtime, locale);
  return line && line !== "-" ? line : "";
}

function uniqueProviderCapabilities(source: ShareIdentitySource) {
  const enabledApps = new Set(source.supportedApps || source.apps || []);
  const capabilities = (source.appCapabilities || []).filter((item) => {
    const hasProvider = !!(item.providerName?.trim() || item.providerType?.trim() || item.kind?.trim());
    return hasProvider && isCoreShareApp(item.app) && (!enabledApps.size || enabledApps.has(item.app));
  });
  return capabilities.reduce<ShareIdentityProvider[]>((result, capability) => {
    const logo = resolveShareProviderLogo(capability);
    const key = logo?.key || capability.providerType || capability.kind || capability.providerName || capability.app;
    if (!result.some((item) => {
      const itemLogo = resolveShareProviderLogo(item);
      return (itemLogo?.key || item.providerType || item.kind || item.providerName || item.app) === key;
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
        <span className="min-w-0 truncate text-[11px] font-normal text-slate-400" title={statusLine}>
          {statusLine}
        </span>
      ) : null}
    </span>
  );
}
