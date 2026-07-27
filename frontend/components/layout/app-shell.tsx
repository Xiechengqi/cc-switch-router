"use client";

import Image from "next/image";
import Link from "next/link";
import { usePathname, useRouter } from "next/navigation";
import { Button, Dropdown, Toast } from "@heroui/react";
import { Activity, ChevronDown, KeyRound, LogOut, Monitor, Network, Settings, Store, UserRound } from "lucide-react";
import * as React from "react";
import { LoginDialog } from "@/components/auth/login-dialog";
import { useAuth } from "@/components/auth/auth-provider";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { refreshAccessToken } from "@/lib/auth";
import { DashboardDataProvider } from "@/components/dashboard/dashboard-data";
import { AnnouncementDialog } from "@/components/announcement/announcement-dialog";
import {
  DASHBOARD_ACCOUNT_API_KEYS_PATH,
  DASHBOARD_ACCOUNT_PATH,
  DASHBOARD_CLIENTS_PATH,
  DASHBOARD_MARKETS_PATH,
  DASHBOARD_CLIENT_MARKET_PATH,
  type DashboardShellActive,
} from "@/lib/dashboard-nav";
import { cn } from "@/lib/utils";

type RegionOption = {
  name: string;
  url: string;
};

type RegionsCache = {
  regions: RegionOption[];
  selected: string;
};

/** Survives RouterSwitcher remounts during soft/hard navigations. */
let regionsMemoryCache: RegionsCache | null = null;

function normalizeRegionUrl(url: string) {
  const trimmed = url.trim();
  if (!trimmed) return "";
  return /^[a-z][a-z0-9+.-]*:\/\//i.test(trimmed) ? trimmed : `https://${trimmed}`;
}

function currentRegionName(regions: RegionOption[]) {
  if (typeof window === "undefined") return "";
  const hostname = window.location.hostname || "";
  const matched = regions.find((region) => {
    try {
      return new URL(normalizeRegionUrl(region.url)).hostname === hostname;
    } catch {
      return false;
    }
  });
  return matched?.name || "";
}

function sameRouterDomainClientRedirect(raw: string | null) {
  if (!raw || typeof window === "undefined") return null;
  try {
    const target = new URL(raw);
    const current = window.location;
    if (!["http:", "https:"].includes(target.protocol)) return null;
    if (target.hostname === current.hostname) return null;
    if (!target.hostname.endsWith(`.${current.hostname}`)) return null;
    return target.toString();
  } catch {
    return null;
  }
}

function RouterSwitcher() {
  const [regions, setRegions] = React.useState<RegionOption[]>(() => regionsMemoryCache?.regions || []);
  const [selected, setSelected] = React.useState(() => regionsMemoryCache?.selected || "");
  const { t } = useLocaleText();

  React.useEffect(() => {
    let cancelled = false;
    async function load() {
      const response = await fetch("/v1/regions", { cache: "no-store" });
      if (!response.ok || cancelled) return;
      const nextRegions = (await response.json()) as RegionOption[];
      const nextSelected = currentRegionName(nextRegions) || nextRegions[0]?.name || "";
      regionsMemoryCache = { regions: nextRegions, selected: nextSelected };
      if (cancelled) return;
      setRegions(nextRegions);
      setSelected(nextSelected);
    }
    load().catch(console.error);
    return () => {
      cancelled = true;
    };
  }, []);

  if (regions.length === 0) return null;

  const openRegion = (name: string) => {
    if (!name || name === selected) return;
    setSelected(name);
    const region = regions.find((item) => item.name === name);
    const href = region ? normalizeRegionUrl(region.url) : "";
    if (href) window.location.href = href;
  };

  return (
    <Dropdown>
      <Dropdown.Trigger
        aria-label={t("nav.router")}
        className="region-switcher-trigger inline-flex max-w-[11rem] shrink-0 items-center gap-1 outline-none"
      >
        <span className="min-w-0 truncate">{selected || t("nav.router")}</span>
        <ChevronDown className="h-4 w-4 shrink-0 text-slate-400" aria-hidden />
      </Dropdown.Trigger>
      <Dropdown.Popover placement="bottom start" className="min-w-[8.5rem]">
        <Dropdown.Menu aria-label={t("nav.routers")}>
          {regions.map((region) => (
            <Dropdown.Item
              key={region.name}
              id={region.name}
              className={cn(
                "text-sm",
                region.name === selected ? "font-semibold" : "font-medium",
              )}
              onAction={() => openRegion(region.name)}
            >
              {region.name}
            </Dropdown.Item>
          ))}
        </Dropdown.Menu>
      </Dropdown.Popover>
    </Dropdown>
  );
}

function LanguageSwitcher() {
  const { locale, setLocale, t } = useLocaleText();
  const showEnglish = locale !== "en";

  return (
    <button
      type="button"
      aria-label={t("common.language")}
      className="inline-flex h-9 shrink-0 items-center justify-center rounded-md px-2 text-sm font-medium tracking-wide text-slate-400 transition-colors hover:text-slate-600"
      onClick={() => setLocale(showEnglish ? "en" : "zh-CN")}
    >
      {showEnglish ? "EN" : "中"}
    </button>
  );
}

function DashboardNav({
  active,
  authed,
}: {
  active: "clients" | "markets" | "client-market" | "account";
  authed: boolean;
}) {
  const { t } = useLocaleText();
  const pathname = usePathname() || DASHBOARD_CLIENTS_PATH;
  const selectedKey =
    authed && (active === "account" || pathname.startsWith("/account"))
      ? "account"
      : active === "client-market" || pathname.startsWith("/client-market")
        ? "client-market"
        : active === "markets" || pathname.startsWith("/markets")
          ? "markets"
          : "clients";

  const items = [
    { id: "clients" as const, href: DASHBOARD_CLIENTS_PATH, icon: Monitor, label: t("nav.clientsTab") },
    { id: "markets" as const, href: DASHBOARD_MARKETS_PATH, icon: Store, label: t("nav.marketsTab") },
    {
      id: "client-market" as const,
      href: DASHBOARD_CLIENT_MARKET_PATH,
      icon: Network,
      label: t("nav.clientMarketTab"),
    },
    ...(authed
      ? [{ id: "account" as const, href: DASHBOARD_ACCOUNT_PATH, icon: UserRound, label: t("nav.accountTab") }]
      : []),
  ];

  return (
    <nav aria-label={t("nav.dashboardSections")} className="flex w-max max-w-full items-center gap-0.5">
      {items.map((item) => {
        const selected = selectedKey === item.id;
        const Icon = item.icon;
        return (
          <Link
            key={item.id}
            href={item.href}
            aria-current={selected ? "page" : undefined}
            className={cn(
              "inline-flex h-9 min-w-0 items-center gap-1.5 rounded-md px-2.5 text-sm transition-colors",
              selected
                ? "font-semibold text-slate-600"
                : "font-medium text-slate-400 hover:text-slate-600",
            )}
          >
            <Icon
              className={cn(
                "h-4 w-4 shrink-0",
                selected ? "text-slate-500" : "text-slate-400",
              )}
              aria-hidden
            />
            <span className="whitespace-nowrap">{item.label}</span>
          </Link>
        );
      })}
    </nav>
  );
}

function Topbar({ active }: { active: DashboardShellActive }) {
  const { session, loading, logout } = useAuth();
  const { t } = useLocaleText();
  const router = useRouter();
  const [loginOpen, setLoginOpen] = React.useState(false);
  const [clientRedirect, setClientRedirect] = React.useState<string | null>(null);
  const redirectStartedRef = React.useRef(false);
  const lastAuthedEmailRef = React.useRef<string | null>(null);
  if (session?.authenticated && session.user?.email) {
    lastAuthedEmailRef.current = session.user.email;
  } else if (session && !session.authenticated) {
    lastAuthedEmailRef.current = null;
  }
  const authed = !!session?.authenticated;
  const showAuthedChrome = authed || (loading && !!lastAuthedEmailRef.current);
  const displayEmail = session?.user?.email || lastAuthedEmailRef.current || "";
  const showDashboardNav = active === "clients" || active === "markets" || active === "client-market" || active === "account";

  React.useEffect(() => {
    setClientRedirect(sameRouterDomainClientRedirect(new URLSearchParams(window.location.search).get("clientRedirect")));
  }, []);

  React.useEffect(() => {
    if (!clientRedirect || loading || authed) return;
    setLoginOpen(true);
  }, [authed, clientRedirect, loading]);

  // P18: ShareConnectDialog 在未登录态点击「登录」时派发 router-open-login，
  // 由 Topbar 统一接住打开 LoginDialog。和 router-auth-changed 同模式（见
  // AuthProvider）。
  React.useEffect(() => {
    const handler = () => setLoginOpen(true);
    window.addEventListener("router-open-login", handler);
    return () => window.removeEventListener("router-open-login", handler);
  }, []);

  React.useEffect(() => {
    if (!clientRedirect || loading || !authed || redirectStartedRef.current) return;
    redirectStartedRef.current = true;
    refreshAccessToken()
      .catch(() => false)
      .finally(() => {
        window.location.replace(clientRedirect);
      });
  }, [authed, clientRedirect, loading]);

  return (
    <>
      {/*
        Three-column topbar: brand (logo + region title) | centered section tabs | lang + user.
        Region is a chrome-less title dropdown (font-display + chevron), not a bordered Select.
      */}
      <div className="sticky top-0 z-40 bg-background/85 backdrop-blur-md">
        <header className="mx-auto w-[calc(100%-2rem)] max-w-7xl py-3.5">
        <div className="grid grid-cols-[minmax(0,1fr)_auto_minmax(0,1fr)] items-center gap-x-3 gap-y-2">
          <div className="flex min-w-0 items-center gap-2.5 justify-self-start">
            <Link href={DASHBOARD_CLIENTS_PATH} className="flex shrink-0 items-center" aria-label="CC-Switch Router">
              <Image src="/router-logo.svg" alt="" width={32} height={32} className="h-8 w-8 shrink-0" priority />
            </Link>
            <RouterSwitcher />
          </div>

          {showDashboardNav ? (
            <div className="min-w-0 justify-self-center overflow-x-auto [-ms-overflow-style:none] [scrollbar-width:none] [&::-webkit-scrollbar]:hidden">
              <DashboardNav active={active} authed={showAuthedChrome} />
            </div>
          ) : (
            <div />
          )}

          <div className="flex flex-nowrap items-center justify-end gap-2 justify-self-end">
            <LanguageSwitcher />
            {showAuthedChrome ? (
              <Dropdown>
                <Dropdown.Trigger className="shrink-0 outline-none">
                  <Button
                    variant="ghost"
                    size="sm"
                    className="h-9 max-w-[12rem] gap-1.5 px-2.5 text-sm font-medium text-slate-400 hover:text-slate-600 whitespace-nowrap [&_svg]:my-0"
                    isDisabled={loading && !authed}
                  >
                    <UserRound className="h-4 w-4 shrink-0 text-slate-400" />
                    <span className="hidden min-w-0 truncate sm:inline">{displayEmail}</span>
                  </Button>
                </Dropdown.Trigger>
                <Dropdown.Popover placement="bottom right">
                  <Dropdown.Menu aria-label={t("nav.userMenu")}>
                    <Dropdown.Section>
                      <Dropdown.Item id="email" isDisabled className="text-xs text-muted-foreground">
                        {displayEmail}
                      </Dropdown.Item>
                    </Dropdown.Section>
                    <Dropdown.Item id="api-token" onAction={() => router.push(DASHBOARD_ACCOUNT_API_KEYS_PATH)}>
                      <KeyRound className="h-4 w-4" />
                      API Token
                    </Dropdown.Item>
                    {session?.isAdmin ? (
                      <>
                        <Dropdown.Item id="metrics" onAction={() => window.open("/metrics/", "_blank", "noopener,noreferrer")}>
                          <Activity className="h-4 w-4" />
                          {t("nav.metrics")}
                        </Dropdown.Item>
                        <Dropdown.Item id="settings" onAction={() => window.open("/settings/", "_blank", "noopener,noreferrer")}>
                          <Settings className="h-4 w-4" />
                          {t("nav.settings")}
                        </Dropdown.Item>
                      </>
                    ) : null}
                    <Dropdown.Item id="logout" onAction={() => logout().catch(console.error)} className="text-destructive">
                      <LogOut className="h-4 w-4" />
                      {t("nav.logout")}
                    </Dropdown.Item>
                  </Dropdown.Menu>
                </Dropdown.Popover>
              </Dropdown>
            ) : (
              <Button
                variant="ghost"
                size="sm"
                className="h-9 shrink-0 px-2.5 text-sm font-medium text-slate-400 hover:text-slate-600"
                onClick={() => setLoginOpen(true)}
                isDisabled={loading}
              >
                {t("nav.login")}
              </Button>
            )}
          </div>
        </div>
        </header>
      </div>

      <LoginDialog open={loginOpen} onOpenChange={setLoginOpen} />
    </>
  );
}

export function AppShell({
  active,
  children,
}: {
  active: DashboardShellActive;
  children: React.ReactNode;
}) {
  const dashboardDataEnabled = active === "clients" || active === "markets";
  return (
    <DashboardDataProvider enabled={dashboardDataEnabled}>
      <div className="flex min-h-dvh min-w-0 flex-col">
        <Topbar active={active} />
        <div className="flex min-w-0 flex-1 flex-col pt-4 sm:pt-5">{children}</div>
      </div>
      <AnnouncementDialog />
      <Toast.Provider placement="top end" />
    </DashboardDataProvider>
  );
}
