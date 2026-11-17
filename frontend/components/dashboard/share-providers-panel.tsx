"use client";

import * as React from "react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import type { AppLocale } from "@/lib/i18n";
import type { ShareAppProvider, ShareAppRuntimes, ShareUpstreamProvider, ShareView } from "@/lib/types";
import { shareEnabledApps, SHARE_APP_LABELS } from "@/lib/share-app";
import { EmptyBlock } from "./empty-block";
import { ShareProviderStatusPanel } from "./share-provider-status-panel";
import {
  boundProviderIdForApp,
  isApiProviderRuntime,
  mergeStandaloneOAuthRuntime,
  modelHealthTitle,
  modelHealthTone,
  providerActualModelNames,
  providerApiEndpoint,
  providerQuotaDetails,
  providerQuotaStatusLine,
  providerStatusIdentity,
  shareAppProviderRuntime,
  type CoreShareApp,
  type TFn,
} from "./share-dashboard-utils";

const PROVIDER_APPS: Array<{ key: CoreShareApp; label: string }> = [
  { key: "claude", label: "Claude" },
  { key: "codex", label: "Codex" },
  { key: "gemini", label: "Gemini" },
];

type GroupedProvider = {
  key: string;
  providers: Partial<Record<CoreShareApp, ShareAppProvider>>;
  sources: Partial<Record<CoreShareApp, ShareView>>;
  supportedApps: CoreShareApp[];
};

function normalizeProviderApps(
  provider: ShareAppProvider,
  observedApps: CoreShareApp[] = [],
): CoreShareApp[] {
  const values = new Set(
    [...(provider.supportedApps || []), provider.app, ...observedApps].map(
      (app) => String(app).trim().toLowerCase(),
    ),
  );
  return PROVIDER_APPS.map((item) => item.key).filter((app) => values.has(app));
}

function providerGroupKey(provider: ShareAppProvider, app: CoreShareApp) {
  const bundleId = String(provider.bundleId || "").trim();
  return bundleId ? `bundle:${bundleId}` : `surface:${app}:${provider.id}`;
}

function groupProviders(
  entries: Array<{ app: CoreShareApp; provider: ShareAppProvider; source?: ShareView }>,
): GroupedProvider[] {
  const groups = new Map<string, GroupedProvider>();
  entries.forEach(({ app, provider, source }) => {
    const revealAccount = source?.canViewAccountIdentity ?? source?.canManage === true;
    const key = `${providerGroupKey(provider, app)}:${revealAccount ? "full" : "masked"}`;
    const existing = groups.get(key);
    const supportedApps = normalizeProviderApps(provider, [app]);
    if (!existing) {
      groups.set(key, {
        key,
        providers: { [app]: provider },
        sources: source ? { [app]: source } : {},
        supportedApps,
      });
      return;
    }
    existing.providers[app] = provider;
    if (source) existing.sources[app] = source;
    existing.supportedApps = normalizeProviderApps(provider, [
      ...existing.supportedApps,
      ...supportedApps,
    ]);
  });
  return [...groups.values()].sort((left, right) => {
    const leftProvider = PROVIDER_APPS.map(
      ({ key }) => left.providers[key],
    ).find(Boolean);
    const rightProvider = PROVIDER_APPS.map(
      ({ key }) => right.providers[key],
    ).find(Boolean);
    const leftName = leftProvider?.name || leftProvider?.id || left.key;
    const rightName = rightProvider?.name || rightProvider?.id || right.key;
    const byName = leftName.localeCompare(rightName);
    return byName || left.key.localeCompare(right.key);
  });
}

export function providerRuntime(
  provider: ShareAppProvider,
): ShareUpstreamProvider {
  return shareAppProviderRuntime(provider);
}

export function providerMetaLabel(provider: ShareAppProvider) {
  return [provider.kind, provider.providerType].filter(Boolean).join(" · ");
}

function shareProviderStatusView({
  share,
  providers,
  sources,
  appRuntimes,
  t,
  locale,
}: {
  share?: ShareView;
  providers: Partial<Record<CoreShareApp, ShareAppProvider>>;
  sources?: Partial<Record<CoreShareApp, ShareView>>;
  appRuntimes: ShareAppRuntimes | undefined;
  t: TFn;
  locale: AppLocale;
}) {
  const entries = PROVIDER_APPS.flatMap(({ key: app }) => {
    const provider = providers[app];
    if (!provider) return [];
    return [
      {
        app,
        runtime: mergeStandaloneOAuthRuntime(
          providerRuntime(provider),
          sources?.[app]?.appRuntimes || appRuntimes,
          provider,
        ),
      },
    ];
  });
  const primary = entries[0];
  if (!primary) return null;
  const { runtime } = primary;
  const quotaStatusLine = providerQuotaStatusLine(runtime, locale);
  const source = sources?.[primary.app] || share;
  const accountLine = providerStatusIdentity(runtime, source?.canViewAccountIdentity ?? source?.canManage === true);
  const modelsLine =
    entries
      .map(
        (entry) =>
          `${SHARE_APP_LABELS[entry.app]}: ${providerActualModelNames(
            entry.runtime,
            t("dashboard.modelPolicyPassthrough"),
          )}`,
      )
      .join(" · ") || "-";
  const isApiProvider = isApiProviderRuntime(runtime);
  const apiEndpoint = providerApiEndpoint(runtime);
  const healthTone = share
    ? modelHealthTone(share, undefined, locale)
    : { className: "bg-slate-50 text-muted-foreground", label: "" };
  return {
    primaryLine: isApiProvider ? apiEndpoint : quotaStatusLine,
    identityLine: isApiProvider ? "-" : accountLine,
    modelsLine,
    primaryTitle: isApiProvider
      ? `${t("dashboard.apiRequestUrl")}: ${apiEndpoint}`
      : providerQuotaDetails(runtime, locale),
    identityTitle: isApiProvider ? "-" : accountLine,
    modelsTitle: modelsLine,
    panelTitle: share ? modelHealthTitle(share, undefined, locale) : undefined,
    primaryMonospace: isApiProvider,
    toneClassName: healthTone.className,
    healthLabel: "detailLabel" in healthTone && typeof healthTone.detailLabel === "string" ? healthTone.detailLabel : undefined,
  };
}

export function ShareProvidersPanel({ share }: { share?: ShareView }) {
  const { locale, t } = useLocaleText();
  const runtimes = share?.appRuntimes;
  const currentProviders = React.useMemo(() => {
    if (!share) return [];
    const entries = shareEnabledApps(share).flatMap((app) => {
      const providers = share.appProviders?.[app] || [];
      const boundProviderId = boundProviderIdForApp(share, app);
      const provider =
        providers.find((item) => item.id === boundProviderId) ||
        providers.find((item) => item.isCurrent);
      return provider ? [{ app, provider, source: share }] : [];
    });
    return groupProviders(entries);
  }, [share]);

  return (
    <div className="grid gap-3">
      {!currentProviders.length ? (
        <EmptyBlock>{t("dashboard.noProviders")}</EmptyBlock>
      ) : (
        <div className="grid gap-2">
          {currentProviders.map((item) => {
            const view = shareProviderStatusView({
              share,
              providers: item.providers,
              sources: item.sources,
              appRuntimes: runtimes,
              t,
              locale,
            });
            return view ? (
              <ShareProviderStatusPanel
                key={item.key}
                view={view}
                wrapPrimaryLine
              />
            ) : null;
          })}
        </div>
      )}
    </div>
  );
}

/** Client 侧边栏按 Server Provider Bundle 身份合并供应商；独立 Surface 不跨 App 猜测合并。 */
export function ClientProvidersPanel({ shares }: { shares: ShareView[] }) {
  const { locale, t } = useLocaleText();

  const merged = React.useMemo(() => {
    const entries: Array<{ app: CoreShareApp; provider: ShareAppProvider; source: ShareView }> =
      [];
    shares.forEach((share) => {
      PROVIDER_APPS.forEach(({ key: app }) => {
        (share.appProviders?.[app] || []).forEach((provider) => {
          entries.push({ app, provider, source: share });
        });
      });
    });
    return groupProviders(entries);
  }, [shares]);

  return (
    <div className="grid gap-3">
      {!merged.length ? (
        <EmptyBlock>{t("dashboard.noProviders")}</EmptyBlock>
      ) : (
        <div className="grid gap-2">
          {merged.map((item) => {
            const share = PROVIDER_APPS.map(({ key }) => item.sources[key]).find(Boolean);
            const view = shareProviderStatusView({
              share,
              providers: item.providers,
              sources: item.sources,
              appRuntimes: undefined,
              t,
              locale,
            });
            return view ? (
              <ShareProviderStatusPanel
                key={item.key}
                view={view}
                wrapPrimaryLine
              />
            ) : null;
          })}
        </div>
      )}
    </div>
  );
}
