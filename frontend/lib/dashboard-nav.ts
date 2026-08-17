export const DASHBOARD_CLIENTS_PATH = "/clients/";
export const DASHBOARD_MARKETS_PATH = "/markets/";
export const DASHBOARD_SHARE_MARKET_PATH = "/share-market/";
export const DASHBOARD_CLIENT_MARKET_PATH = "/client-market/";
export const DASHBOARD_ACCOUNT_PATH = "/account/";
export const DASHBOARD_ACCOUNT_API_KEYS_PATH = "/account/api-keys/";
export const DASHBOARD_ACCOUNT_NOTIFICATIONS_PATH = "/account/notifications/";
export const DASHBOARD_ACCOUNT_PROVIDER_USAGE_PATH = "/account/provider-usage/";
export const DASHBOARD_ACCOUNT_CONSUMER_USAGE_PATH = "/account/consumer-usage/";
export const DASHBOARD_ACCOUNT_MARKET_READINESS_PATH = "/account/market-readiness/";
export const DASHBOARD_ACCOUNT_BILLING_PATH = "/account/billing/";
export const DASHBOARD_ACCOUNT_MARKET_ACCESS_PATH = "/account/market-access/";
export const DASHBOARD_ACCOUNT_PAYMENTS_PATH = "/account/payments/";
export const DASHBOARD_ACCOUNT_SHARE_PATH = "/account/share/";
export const DASHBOARD_ACCOUNT_CLIENT_PATH = "/account/client/";
/** @deprecated Prefer DASHBOARD_ACCOUNT_CLIENT_PATH; kept for redirect matching. */
export const DASHBOARD_ACCOUNT_RENTALS_PATH = "/account/rentals/";

export type DashboardRoute =
  | typeof DASHBOARD_CLIENTS_PATH
  | typeof DASHBOARD_MARKETS_PATH
  | typeof DASHBOARD_SHARE_MARKET_PATH
  | typeof DASHBOARD_CLIENT_MARKET_PATH
  | typeof DASHBOARD_ACCOUNT_PATH
  | typeof DASHBOARD_ACCOUNT_API_KEYS_PATH
  | typeof DASHBOARD_ACCOUNT_NOTIFICATIONS_PATH
  | typeof DASHBOARD_ACCOUNT_PROVIDER_USAGE_PATH
  | typeof DASHBOARD_ACCOUNT_CONSUMER_USAGE_PATH
  | typeof DASHBOARD_ACCOUNT_MARKET_READINESS_PATH
  | typeof DASHBOARD_ACCOUNT_BILLING_PATH
  | typeof DASHBOARD_ACCOUNT_MARKET_ACCESS_PATH
  | typeof DASHBOARD_ACCOUNT_PAYMENTS_PATH
  | typeof DASHBOARD_ACCOUNT_SHARE_PATH
  | typeof DASHBOARD_ACCOUNT_CLIENT_PATH
  | typeof DASHBOARD_ACCOUNT_RENTALS_PATH;
export type DashboardShellActive = "clients" | "markets" | "share-market" | "client-market" | "account" | "settings" | "operations" | "metrics";

export function normalizeDashboardPath(pathname: string): DashboardRoute | null {
  if (pathname.startsWith("/account/rentals")) return DASHBOARD_ACCOUNT_RENTALS_PATH;
  if (pathname.startsWith("/account/client")) return DASHBOARD_ACCOUNT_CLIENT_PATH;
  if (pathname.startsWith("/account/share")) return DASHBOARD_ACCOUNT_SHARE_PATH;
  if (pathname.startsWith("/account/market-readiness")) return DASHBOARD_ACCOUNT_MARKET_READINESS_PATH;
  if (pathname.startsWith("/account/billing")) return DASHBOARD_ACCOUNT_BILLING_PATH;
  if (pathname.startsWith("/account/market-access")) return DASHBOARD_ACCOUNT_MARKET_ACCESS_PATH;
  if (pathname.startsWith("/account/payments")) return DASHBOARD_ACCOUNT_PAYMENTS_PATH;
  if (pathname.startsWith("/account/provider-usage")) return DASHBOARD_ACCOUNT_PROVIDER_USAGE_PATH;
  if (pathname.startsWith("/account/consumer-usage")) return DASHBOARD_ACCOUNT_CONSUMER_USAGE_PATH;
  if (pathname.startsWith("/account/api-keys")) return DASHBOARD_ACCOUNT_API_KEYS_PATH;
  if (pathname.startsWith("/account/notifications")) return DASHBOARD_ACCOUNT_NOTIFICATIONS_PATH;
  if (pathname.startsWith("/account")) return DASHBOARD_ACCOUNT_PATH;
  if (pathname.startsWith("/client-market")) return DASHBOARD_CLIENT_MARKET_PATH;
  if (pathname.startsWith("/share-market")) return DASHBOARD_SHARE_MARKET_PATH;
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
  if (pathname.startsWith("/account")) return "account";
  if (pathname.startsWith("/client-market")) return "client-market";
  if (pathname.startsWith("/share-market")) return "share-market";
  if (pathname.startsWith("/markets")) return "markets";
  if (pathname.startsWith("/clients")) return "clients";
  if (pathname.startsWith("/metrics")) return "metrics";
  if (pathname.startsWith("/operations")) return "operations";
  if (pathname.startsWith("/settings")) return "settings";
  return "clients";
}

export function isClientsRoute(pathname: string) {
  return pathname.startsWith("/clients");
}

export function isMarketsRoute(pathname: string) {
  return pathname.startsWith("/markets");
}

export function isClientMarketRoute(pathname: string) {
  return pathname.startsWith("/client-market");
}

export function isShareMarketRoute(pathname: string) {
  return pathname.startsWith("/share-market");
}

export const ACCOUNT_NAV_STORAGE_KEY = "cc_switch_router_account_nav_v1";

const ACCOUNT_NAV_HREFS = [
  DASHBOARD_ACCOUNT_API_KEYS_PATH,
  DASHBOARD_ACCOUNT_NOTIFICATIONS_PATH,
  DASHBOARD_ACCOUNT_PROVIDER_USAGE_PATH,
  DASHBOARD_ACCOUNT_CONSUMER_USAGE_PATH,
  DASHBOARD_ACCOUNT_MARKET_READINESS_PATH,
  DASHBOARD_ACCOUNT_BILLING_PATH,
  DASHBOARD_ACCOUNT_MARKET_ACCESS_PATH,
  DASHBOARD_ACCOUNT_PAYMENTS_PATH,
  DASHBOARD_ACCOUNT_SHARE_PATH,
  DASHBOARD_ACCOUNT_CLIENT_PATH,
] as const;

export type AccountNavHref = (typeof ACCOUNT_NAV_HREFS)[number];

export function isAccountNavHref(value: string | null | undefined): value is AccountNavHref {
  return !!value && (ACCOUNT_NAV_HREFS as readonly string[]).includes(value);
}

export function accountNavHrefFromPathname(pathname: string): AccountNavHref | null {
  const normalized = normalizeDashboardPath(pathname);
  if (normalized === DASHBOARD_ACCOUNT_RENTALS_PATH) return DASHBOARD_ACCOUNT_CLIENT_PATH;
  return isAccountNavHref(normalized) ? normalized : null;
}

export function readStoredAccountNavHref(): AccountNavHref | null {
  if (typeof localStorage === "undefined") return null;
  try {
    const raw = localStorage.getItem(ACCOUNT_NAV_STORAGE_KEY);
    if (raw === DASHBOARD_ACCOUNT_RENTALS_PATH) return DASHBOARD_ACCOUNT_CLIENT_PATH;
    return isAccountNavHref(raw) ? raw : null;
  } catch {
    return null;
  }
}

export function writeStoredAccountNavHref(href: AccountNavHref) {
  if (typeof localStorage === "undefined") return;
  try {
    localStorage.setItem(ACCOUNT_NAV_STORAGE_KEY, href);
  } catch {
    // ignore quota / private mode
  }
}

export function resolveAccountEntryHref(): AccountNavHref {
  return readStoredAccountNavHref() || DASHBOARD_ACCOUNT_API_KEYS_PATH;
}

/** Deep-link into Client Market「我的」, optionally focusing a rental row. */
export function clientMarketMineHref(installationId?: string) {
  const params = new URLSearchParams({ tab: "mine" });
  if (installationId) params.set("focus", installationId);
  return buildDashboardHref(DASHBOARD_CLIENT_MARKET_PATH, params);
}

export type ShareMarketWorkspaceParam = "catalog" | "selling";

/** Deep-link into Share Market, optionally selecting a workspace and focusing a share. */
export function shareMarketHref(options?: { workspace?: ShareMarketWorkspaceParam; shareId?: string }) {
  const params = new URLSearchParams();
  if (options?.workspace === "selling") params.set("view", "selling");
  if (options?.shareId) params.set("focus", options.shareId);
  return buildDashboardHref(DASHBOARD_SHARE_MARKET_PATH, params);
}
