import { buildDashboardHref, defaultDashboardRouteFromSearch, DASHBOARD_CLIENTS_PATH } from "@/lib/dashboard-nav";

/** The Router home is a destination, not an account preference dispatcher. */
export function rootTaskHref(search: string): string | null {
  const destination = defaultDashboardRouteFromSearch(search);
  return destination === DASHBOARD_CLIENTS_PATH ? null : buildDashboardHref(destination, search);
}

export function legacyClientsHref(pathname: string, search = "", hash = ""): string | null {
  return pathname === "/clients" || pathname === "/clients/" ? `/${search}${hash}` : null;
}

export type UiContext = { mode: "router" } | { mode: "share"; shareId: string; subdomain: string };

export function parseUiContext(value: unknown): UiContext {
  if (!value || typeof value !== "object") throw new Error("Invalid UI context");
  const v = value as Record<string, unknown>;
  if (v.mode === "router") return { mode: "router" };
  if (v.mode === "share" && typeof v.shareId === "string" && v.shareId && typeof v.subdomain === "string" && v.subdomain) {
    return { mode: "share", shareId: v.shareId, subdomain: v.subdomain };
  }
  throw new Error("Invalid UI context");
}

export async function getUiContext(signal?: AbortSignal): Promise<UiContext> {
  const timeout = AbortSignal.timeout(10_000);
  const response = await fetch("/v1/ui/context", { cache: "no-store", signal: signal ? AbortSignal.any([signal, timeout]) : timeout });
  if (!response.ok) throw new Error("UI context unavailable");
  return parseUiContext(await response.json());
}
