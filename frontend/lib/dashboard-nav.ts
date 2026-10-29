export const DASHBOARD_CLIENTS_PATH = "/clients/";
export const DASHBOARD_SHARE_MARKET_PATH = "/share-market/";
export const DASHBOARD_CLIENT_MARKET_PATH = "/client-market/";
export const DASHBOARD_PROVIDERS_PATH = "/providers/";
export const DASHBOARD_ACCOUNT_PATH = "/account/";
export const DASHBOARD_ACCOUNT_WALLET_PATH = "/account/wallet/";
/** The balance page section the navbar balance focuses when that page is open. */
export const WALLET_ACTIONS_ID = "wallet-actions";
/** What the account rents from both markets. */
export const DASHBOARD_ACCOUNT_RENTALS_PATH = "/account/rentals/";
export const DASHBOARD_ACCOUNT_PROVIDER_SETTINGS_PATH = "/account/provider-settings/";
export const DASHBOARD_ACCOUNT_USAGE_PATH = "/account/usage/";
export const DASHBOARD_ACCOUNT_SECURITY_PATH = "/account/security/";
export const DASHBOARD_ACCOUNT_NOTIFICATIONS_PATH = "/account/notifications/";
/** The security page card that holds the home entry choice. */
export const HOME_ENTRY_SECTION_ID = "home-entry";
export const DASHBOARD_ACCOUNT_HOME_HREF = `${DASHBOARD_ACCOUNT_SECURITY_PATH}#${HOME_ENTRY_SECTION_ID}`;

export const PROVIDER_INCOME_HREF = `${DASHBOARD_ACCOUNT_PROVIDER_SETTINGS_PATH}?tab=income`;
export const PROVIDER_ACCESS_HREF = `${DASHBOARD_ACCOUNT_PROVIDER_SETTINGS_PATH}?tab=access`;
export const PROVIDER_CENTER_TABS = ["listings", "income", "access", "contact"] as const;
export type ProviderCenterTab = (typeof PROVIDER_CENTER_TABS)[number];

/** The Provider center tab from `?tab=`, accepting the older `#income` anchor. */
export function providerCenterTab(tab: string | null | undefined, hash = ""): ProviderCenterTab {
  if (PROVIDER_CENTER_TABS.includes(tab as ProviderCenterTab)) return tab as ProviderCenterTab;
  const anchor = hash.replace(/^#/, "");
  return PROVIDER_CENTER_TABS.includes(anchor as ProviderCenterTab) ? anchor as ProviderCenterTab : "listings";
}

/** Which market a rentals or listings view shows; both when absent. */
export const ACCOUNT_MARKET_FILTERS = ["all", "share", "client"] as const;
export type AccountMarketFilter = (typeof ACCOUNT_MARKET_FILTERS)[number];

export function accountMarketFilter(value: string | null | undefined): AccountMarketFilter {
  return ACCOUNT_MARKET_FILTERS.includes(value as AccountMarketFilter) ? value as AccountMarketFilter : "all";
}

export const ACCOUNT_USAGE_VIEWS = ["consumer", "provider"] as const;
export type AccountUsageView = (typeof ACCOUNT_USAGE_VIEWS)[number];

export function accountUsageView(value: string | null | undefined): AccountUsageView {
  return value === "provider" ? "provider" : "consumer";
}

export const NOTIFICATION_SETTINGS_TABS = ["channels", "types", "history"] as const;
export type NotificationSettingsTab = (typeof NOTIFICATION_SETTINGS_TABS)[number];

export function notificationSettingsTab(value: string | null | undefined): NotificationSettingsTab {
  return NOTIFICATION_SETTINGS_TABS.includes(value as NotificationSettingsTab) ? value as NotificationSettingsTab : "channels";
}

/**
 * Where a retired account address now lives. Old emails and bookmarks carry
 * `?tab=user|provider&focus=…` for the per-market pages, so the role picks
 * the page and the market is kept as the filter that scopes the focus.
 */
export function legacyAccountHref(pathname: string, search = ""): string | null {
  const params = new URLSearchParams(search);
  const focus = params.get("focus");
  const path = pathname.replace(/\/+$/, "");
  const withParams = (base: string, entries: [string, string | null][]) => {
    const next = new URLSearchParams();
    for (const [key, value] of entries) if (value) next.set(key, value);
    const query = next.toString();
    return query ? `${base}?${query}` : base;
  };
  for (const market of ["share", "client"] as const) {
    if (path !== `/account/${market}`) continue;
    return params.get("tab") === "provider"
      ? withParams(DASHBOARD_ACCOUNT_PROVIDER_SETTINGS_PATH, [["tab", "listings"], ["market", market], ["focus", focus]])
      : withParams(DASHBOARD_ACCOUNT_RENTALS_PATH, [["market", market], ["focus", focus]]);
  }
  switch (path) {
    case "/account/api-keys": return DASHBOARD_ACCOUNT_SECURITY_PATH;
    case "/account/home": return DASHBOARD_ACCOUNT_HOME_HREF;
    case "/account/consumer-usage": return `${DASHBOARD_ACCOUNT_USAGE_PATH}?view=consumer`;
    case "/account/provider-usage": return `${DASHBOARD_ACCOUNT_USAGE_PATH}?view=provider`;
    case "/account/market-access": return PROVIDER_ACCESS_HREF;
    case "/account/market-readiness": return DASHBOARD_ACCOUNT_PROVIDER_SETTINGS_PATH;
    default: return null;
  }
}

export type DashboardRoute =
  | typeof DASHBOARD_CLIENTS_PATH
  | typeof DASHBOARD_SHARE_MARKET_PATH
  | typeof DASHBOARD_CLIENT_MARKET_PATH
  | typeof DASHBOARD_PROVIDERS_PATH
  | typeof DASHBOARD_ACCOUNT_PATH
  | typeof DASHBOARD_ACCOUNT_WALLET_PATH
  | typeof DASHBOARD_ACCOUNT_RENTALS_PATH
  | typeof DASHBOARD_ACCOUNT_PROVIDER_SETTINGS_PATH
  | typeof DASHBOARD_ACCOUNT_USAGE_PATH
  | typeof DASHBOARD_ACCOUNT_SECURITY_PATH
  | typeof DASHBOARD_ACCOUNT_NOTIFICATIONS_PATH;
export type DashboardShellActive = "clients" | "share-market" | "client-market" | "providers" | "account" | "settings" | "operations" | "metrics";

export function normalizeDashboardPath(pathname: string): DashboardRoute | null {
  if (pathname.startsWith("/account/wallet")) return DASHBOARD_ACCOUNT_WALLET_PATH;
  // Retired per-market pages now live in the rentals list.
  if (pathname.startsWith("/account/rentals") || pathname.startsWith("/account/client") || pathname.startsWith("/account/share")) {
    return DASHBOARD_ACCOUNT_RENTALS_PATH;
  }
  if (pathname.startsWith("/account/provider-settings") || pathname.startsWith("/account/market-readiness")
    || pathname.startsWith("/account/market-access")) return DASHBOARD_ACCOUNT_PROVIDER_SETTINGS_PATH;
  if (pathname.startsWith("/account/usage") || pathname.startsWith("/account/provider-usage")
    || pathname.startsWith("/account/consumer-usage")) return DASHBOARD_ACCOUNT_USAGE_PATH;
  if (pathname.startsWith("/account/security") || pathname.startsWith("/account/api-keys")
    || pathname.startsWith("/account/home")) return DASHBOARD_ACCOUNT_SECURITY_PATH;
  if (pathname.startsWith("/account/notifications")) return DASHBOARD_ACCOUNT_NOTIFICATIONS_PATH;
  if (pathname === "/account" || pathname === DASHBOARD_ACCOUNT_PATH) return DASHBOARD_ACCOUNT_PATH;
  if (pathname.startsWith("/client-market")) return DASHBOARD_CLIENT_MARKET_PATH;
  if (pathname.startsWith("/providers")) return DASHBOARD_PROVIDERS_PATH;
  if (pathname.startsWith("/share-market")) return DASHBOARD_SHARE_MARKET_PATH;
  // Keep old Token Market bookmarks safe, but land them on the built-in
  // Share Market rather than exposing a retired registry state.
  if (pathname.startsWith("/markets")) return DASHBOARD_SHARE_MARKET_PATH;
  if (pathname.startsWith("/clients")) return DASHBOARD_CLIENTS_PATH;
  return null;
}

export function dashboardRouteForDrawer(kind: "client" | "share"): DashboardRoute {
  return DASHBOARD_CLIENTS_PATH;
}

export function dashboardRouteForFocus(kind: "request" | "client" | "share" | "country"): DashboardRoute {
  return DASHBOARD_CLIENTS_PATH;
}

export function defaultDashboardRouteFromSearch(search: string): DashboardRoute {
  const params = new URLSearchParams(search);
  if (params.get("drawerKind") === "market") return DASHBOARD_SHARE_MARKET_PATH;
  if (params.has("topup") || params.has("amount") || params.has("returnTo")) return DASHBOARD_ACCOUNT_WALLET_PATH;
  if (params.has("c2cOrder") || params.get("c2cSell") === "1") return DASHBOARD_ACCOUNT_WALLET_PATH;
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
  if (pathname.startsWith("/providers")) return "providers";
  if (pathname.startsWith("/share-market")) return "share-market";
  if (pathname.startsWith("/markets")) return "share-market";
  if (pathname.startsWith("/clients")) return "clients";
  if (pathname.startsWith("/metrics")) return "metrics";
  if (pathname.startsWith("/operations")) return "operations";
  if (pathname.startsWith("/settings")) return "settings";
  return "clients";
}

export function isClientsRoute(pathname: string) {
  return pathname.startsWith("/clients");
}

export function isClientMarketRoute(pathname: string) {
  return pathname.startsWith("/client-market");
}

export function isShareMarketRoute(pathname: string) {
  return pathname.startsWith("/share-market");
}

export function isProvidersRoute(pathname: string) {
  return pathname.startsWith("/providers");
}

export const ACCOUNT_NAV_STORAGE_KEY = "cc_switch_router_account_nav_v1";

const ACCOUNT_NAV_HREFS = [
  DASHBOARD_ACCOUNT_WALLET_PATH,
  DASHBOARD_ACCOUNT_RENTALS_PATH,
  DASHBOARD_ACCOUNT_PROVIDER_SETTINGS_PATH,
  DASHBOARD_ACCOUNT_USAGE_PATH,
  DASHBOARD_ACCOUNT_SECURITY_PATH,
  DASHBOARD_ACCOUNT_NOTIFICATIONS_PATH,
] as const;

export type AccountNavHref = (typeof ACCOUNT_NAV_HREFS)[number];

export function isAccountNavHref(value: string | null | undefined): value is AccountNavHref {
  return !!value && (ACCOUNT_NAV_HREFS as readonly string[]).includes(value);
}

export function accountNavHrefFromPathname(pathname: string): AccountNavHref | null {
  const normalized = normalizeDashboardPath(pathname);
  return isAccountNavHref(normalized) ? normalized : null;
}

/** The remembered section, mapping addresses stored before sections merged. */
export function readStoredAccountNavHref(): AccountNavHref | null {
  if (typeof localStorage === "undefined") return null;
  try {
    const raw = localStorage.getItem(ACCOUNT_NAV_STORAGE_KEY);
    const href = raw ? accountNavHrefFromPathname(raw) : null;
    if (raw && !href) localStorage.removeItem(ACCOUNT_NAV_STORAGE_KEY);
    return href;
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
  return readStoredAccountNavHref() || DASHBOARD_ACCOUNT_WALLET_PATH;
}

/** Deep-link into Client Market「我的」, optionally focusing a rental row. */
export function clientMarketMineHref(installationId?: string) {
  const params = new URLSearchParams({ tab: "mine" });
  if (installationId) params.set("focus", installationId);
  return buildDashboardHref(DASHBOARD_CLIENT_MARKET_PATH, params);
}

/** Deep-link into Client Market, optionally filtering to one public Market Provider. */
export function clientMarketHref(options?: { providerId?: string }) {
  const params = new URLSearchParams();
  if (options?.providerId) params.set("provider", options.providerId);
  return buildDashboardHref(DASHBOARD_CLIENT_MARKET_PATH, params);
}

export type ShareMarketWorkspaceParam = "catalog" | "rentals" | "selling";

/** Deep-link into Share Market, optionally selecting a workspace and focusing a share. */
export function shareMarketHref(options?: {
  workspace?: ShareMarketWorkspaceParam;
  shareId?: string;
  providerId?: string;
}) {
  const params = new URLSearchParams();
  if (options?.workspace && options.workspace !== "catalog") {
    params.set("view", options.workspace);
  }
  if (options?.shareId) params.set("focus", options.shareId);
  if (options?.providerId) params.set("provider", options.providerId);
  return buildDashboardHref(DASHBOARD_SHARE_MARKET_PATH, params);
}
