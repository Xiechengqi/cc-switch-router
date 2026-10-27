import {
  DASHBOARD_ACCOUNT_WALLET_PATH,
  DASHBOARD_CLIENT_MARKET_PATH,
  DASHBOARD_CLIENTS_PATH,
  DASHBOARD_PROVIDERS_PATH,
  DASHBOARD_SHARE_MARKET_PATH,
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
