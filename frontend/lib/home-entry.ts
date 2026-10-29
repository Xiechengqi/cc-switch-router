import {
  DASHBOARD_ACCOUNT_WALLET_PATH,
  DASHBOARD_CLIENT_MARKET_PATH,
  DASHBOARD_CLIENTS_PATH,
  DASHBOARD_PROVIDERS_PATH,
  DASHBOARD_SHARE_MARKET_PATH,
  buildDashboardHref,
  defaultDashboardRouteFromSearch,
} from "@/lib/dashboard-nav";

export const HOME_ENTRIES = ["clients", "client-market", "share-market", "wallet", "providers"] as const;
export type HomeEntry = (typeof HOME_ENTRIES)[number];
export type HomeEntrySource = "choice" | "skip";

export type HomeEntryStatus = {
  hasActivity: boolean;
  entry: HomeEntry | null;
  source: HomeEntrySource | null;
};

const ENTRY_PATHS: Record<HomeEntry, string> = {
  clients: DASHBOARD_CLIENTS_PATH,
  "client-market": DASHBOARD_CLIENT_MARKET_PATH,
  "share-market": DASHBOARD_SHARE_MARKET_PATH,
  wallet: DASHBOARD_ACCOUNT_WALLET_PATH,
  providers: DASHBOARD_PROVIDERS_PATH,
};

export function isHomeEntry(value: unknown): value is HomeEntry {
  return typeof value === "string" && (HOME_ENTRIES as readonly string[]).includes(value);
}

export function pathForHomeEntry(entry: HomeEntry) {
  return ENTRY_PATHS[entry];
}

export function isGatedHomePath(pathname: string) {
  const path = pathname.replace(/\/+$/, "") || "/";
  return path === "/" || path === "/clients";
}

/** A task address bypasses the start page without changing the saved entry. */
export function isHomeTaskAddress(pathname: string, search: string) {
  if (!isGatedHomePath(pathname)) return true;
  return new URLSearchParams(search).toString().length > 0;
}

export type HomeGateDecision = {
  screen: "content" | "loading" | "start";
  redirect: string | null;
  needsHome: boolean;
};

/** Keep navigation and rendering on the same decision. In particular, both
 * /clients and /clients/ are explicit destinations, never saved-entry redirects.
 * The first-use guide is unchanged; only the bare root applies a saved entry. */
export function resolveHomeGate({ pathname, search, authed, loading, home }: {
  pathname: string;
  search: string | null;
  authed: boolean;
  loading: boolean;
  home: HomeEntryStatus | "failed" | null;
}): HomeGateDecision {
  const path = pathname.replace(/\/+$/, "") || "/";
  const content: HomeGateDecision = { screen: "content", redirect: null, needsHome: false };
  if (!isGatedHomePath(path)) return content;
  if (search === null) return { ...content, screen: "loading" };
  if (isHomeTaskAddress(path, search)) {
    return { ...content, redirect: path === "/" ? buildDashboardHref(defaultDashboardRouteFromSearch(search), search) : null };
  }
  if (loading) return { ...content, screen: "loading" };
  if (!authed) return path === "/" ? { ...content, screen: "loading", redirect: DASHBOARD_CLIENTS_PATH } : content;
  if (home === null) return { screen: "loading", redirect: null, needsHome: true };
  if (home !== "failed" && !home.hasActivity && home.entry === null) return { screen: "start", redirect: null, needsHome: true };
  return {
    screen: path === "/" ? "loading" : "content",
    redirect: path === "/" ? home !== "failed" && home.entry ? pathForHomeEntry(home.entry) : DASHBOARD_CLIENTS_PATH : null,
    needsHome: true,
  };
}

/** A status that can never show the first-use guide again. */
export function isSettledHomeEntryStatus(status: HomeEntryStatus) {
  return status.hasActivity || status.entry !== null;
}

const settledHomeEntryByActor = new Map<string, HomeEntryStatus>();

/** Settled statuses survive soft navigations so returning to Clients never
 * blanks the dashboard shell while the same answer is fetched again. A
 * first-use status is not kept: it is read again on every gated visit. */
export function settledHomeEntryStatus(actor: string) {
  return actor ? settledHomeEntryByActor.get(actor) ?? null : null;
}

export function rememberHomeEntryStatus(actor: string, status: HomeEntryStatus) {
  if (!actor) return;
  if (isSettledHomeEntryStatus(status)) settledHomeEntryByActor.set(actor, status);
  else settledHomeEntryByActor.delete(actor);
}

export function parseHomeEntryStatus(value: unknown): HomeEntryStatus | null {
  if (!value || typeof value !== "object") return null;
  const record = value as Record<string, unknown>;
  if (typeof record.hasActivity !== "boolean") return null;
  if (record.entry !== null && !isHomeEntry(record.entry)) return null;
  if (record.source !== null && record.source !== "choice" && record.source !== "skip") return null;
  return {
    hasActivity: record.hasActivity,
    entry: record.entry,
    source: record.source,
  };
}
