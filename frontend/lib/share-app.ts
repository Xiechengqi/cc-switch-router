import type { ShareView } from "@/lib/types";

export type CoreShareApp = "claude" | "codex" | "gemini";

export const CORE_SHARE_APPS: CoreShareApp[] = ["claude", "codex", "gemini"];

export const SHARE_APP_LABELS: Record<CoreShareApp, string> = {
  claude: "Claude",
  codex: "Codex",
  gemini: "Gemini",
};

export function resolveShareCoreApp(share: ShareView | null | undefined): CoreShareApp | null {
  if (!share) return null;
  const appType = String(share.appType || "").trim().toLowerCase();
  if (appType === "claude" || appType === "codex" || appType === "gemini") {
    return appType;
  }
  const bound = (["claude", "codex", "gemini"] as const).find(
    (app) => typeof share.bindings?.[app] === "string" && share.bindings[app],
  );
  return bound ?? null;
}

export function shareAccessApps(share: ShareView | null | undefined): CoreShareApp[] {
  if (!share) return [];
  const bound = (["claude", "codex", "gemini"] as const).filter(
    (app) => typeof share.bindings?.[app] === "string" && !!share.bindings[app]?.trim(),
  );
  if (bound.length > 0) return bound;
  const app = resolveShareCoreApp(share);
  return app ? [app] : [];
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
