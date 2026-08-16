import type { ShareAppProvider, ShareView } from "@/lib/types";

export type CoreShareApp = "claude" | "codex" | "gemini";

export const CORE_SHARE_APPS: CoreShareApp[] = ["claude", "codex", "gemini"];

export const SHARE_APP_LABELS: Record<CoreShareApp, string> = {
  claude: "Claude",
  codex: "Codex",
  gemini: "Gemini",
};

function asCoreShareApp(value: string | null | undefined): CoreShareApp | null {
  const app = String(value || "").trim().toLowerCase();
  return CORE_SHARE_APPS.includes(app as CoreShareApp) ? (app as CoreShareApp) : null;
}

export function resolveShareCoreApp(share: ShareView | null | undefined): CoreShareApp | null {
  if (!share) return null;
  const fromType = asCoreShareApp(share.appType);
  if (fromType) return fromType;
  return CORE_SHARE_APPS.find(
    (app) => typeof share.bindings?.[app] === "string" && share.bindings[app],
  ) ?? null;
}

export function shareAccessApps(share: ShareView | null | undefined): CoreShareApp[] {
  if (!share) return [];
  const bound = CORE_SHARE_APPS.filter(
    (app) => typeof share.bindings?.[app] === "string" && !!share.bindings[app]?.trim(),
  );
  if (bound.length > 0) return bound;
  const app = resolveShareCoreApp(share);
  return app ? [app] : [];
}

function providerSupportedApps(provider: ShareAppProvider | undefined): CoreShareApp[] {
  if (!provider) return [];
  const fromList = (provider.supportedApps || [])
    .map((app) => asCoreShareApp(app))
    .filter((app): app is CoreShareApp => Boolean(app));
  if (fromList.length > 0) return fromList;
  const fallback = asCoreShareApp(provider.app);
  return fallback ? [fallback] : [];
}

export function shareProviderSupportedApps(share: ShareView | null | undefined): CoreShareApp[] {
  if (!share) return [];
  const seen = new Set<CoreShareApp>();
  const supported: CoreShareApp[] = [];
  for (const app of CORE_SHARE_APPS) {
    const providers = share.appProviders?.[app] || [];
    const current = providers.find((provider) => provider.isCurrent) ?? providers[0];
    for (const supportedApp of providerSupportedApps(current)) {
      if (seen.has(supportedApp)) continue;
      seen.add(supportedApp);
      supported.push(supportedApp);
    }
  }
  return supported.length > 0 ? supported : shareAccessApps(share);
}

export function boundProviderIdForShareApp(share: ShareView | null | undefined, app: CoreShareApp) {
  if (!share) return undefined;
  return share.bindings?.[app] || (share.appType === app ? share.providerId : undefined);
}

export function shareAppApiEnabled(share: ShareView | null | undefined, app: CoreShareApp) {
  if (!shareAccessApps(share).includes(app)) return false;
  const support = share?.support;
  if (!support) return true;
  if (support.claude == null && support.codex == null && support.gemini == null) return true;
  return Boolean(support[app]);
}

export function shareEnabledApps(share: ShareView | null | undefined): CoreShareApp[] {
  return shareAccessApps(share).filter((app) => shareAppApiEnabled(share, app));
}
