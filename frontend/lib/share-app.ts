import type { ShareView } from "@/lib/types";

export type CoreShareApp = "claude" | "codex" | "gemini";

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
  const app = resolveShareCoreApp(share);
  return app ? [app] : [];
}

export function boundProviderIdForShareApp(share: ShareView | null | undefined, app: CoreShareApp) {
  if (!share) return undefined;
  return share.bindings?.[app] || (share.appType === app ? share.providerId : undefined);
}
