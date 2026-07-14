export const DASHBOARD_CLIENTS_PATH = "/clients/";
export const DASHBOARD_MARKETS_PATH = "/markets/";

export type DashboardRoute = typeof DASHBOARD_CLIENTS_PATH | typeof DASHBOARD_MARKETS_PATH;
export type DashboardShellActive = "clients" | "markets" | "settings" | "metrics";

export function normalizeDashboardPath(pathname: string): DashboardRoute | null {
  if (pathname.startsWith("/markets")) return DASHBOARD_MARKETS_PATH;
  if (pathname.startsWith("/clients")) return DASHBOARD_CLIENTS_PATH;
  return null;
}

export function dashboardRouteForDrawer(kind: "client" | "share" | "market"): DashboardRoute {
  return kind === "market" ? DASHBOARD_MARKETS_PATH : DASHBOARD_CLIENTS_PATH;
}

export function dashboardRouteForFocus(kind: "request" | "client" | "share" | "market" | "country"): DashboardRoute {
  return kind === "market" ? DASHBOARD_MARKETS_PATH : DASHBOARD_CLIENTS_PATH;
}

export function defaultDashboardRouteFromSearch(search: string): DashboardRoute {
  const drawerKind = new URLSearchParams(search).get("drawerKind");
  if (drawerKind === "market") return DASHBOARD_MARKETS_PATH;
  return DASHBOARD_CLIENTS_PATH;
}

export function buildDashboardHref(route: DashboardRoute, params?: URLSearchParams | string) {
  if (!params) return route;
  const search = typeof params === "string" ? params : params.toString();
  if (!search) return route;
  return `${route}${search.startsWith("?") ? search : `?${search}`}`;
}

export function pathnameForDashboardShell(pathname: string): DashboardShellActive {
  if (pathname.startsWith("/markets")) return "markets";
  if (pathname.startsWith("/clients")) return "clients";
  if (pathname.startsWith("/metrics")) return "metrics";
  if (pathname.startsWith("/settings")) return "settings";
  return "clients";
}

export function isClientsRoute(pathname: string) {
  return pathname.startsWith("/clients");
}

export function isMarketsRoute(pathname: string) {
  return pathname.startsWith("/markets");
}
