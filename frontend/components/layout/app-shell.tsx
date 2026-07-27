"use client";

import Image from "next/image";
import Link from "next/link";
import { usePathname, useRouter } from "next/navigation";
import { Button, Dropdown, ListBox, Select, Toast } from "@heroui/react";
import { Activity, KeyRound, LogOut, Monitor, Network, Settings, Store, UserRound } from "lucide-react";
import * as React from "react";
import { LoginDialog } from "@/components/auth/login-dialog";
import { AuthProvider, useAuth } from "@/components/auth/auth-provider";
import { LocaleProvider, useLocaleText } from "@/components/i18n/locale-provider";
import { refreshAccessToken } from "@/lib/auth";
import { DashboardDataProvider } from "@/components/dashboard/dashboard-data";
import { AnnouncementDialog } from "@/components/announcement/announcement-dialog";
import {
  DASHBOARD_ACCOUNT_API_KEYS_PATH,
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
  const [regions, setRegions] = React.useState<RegionOption[]>([]);
  const [selected, setSelected] = React.useState("");
  const { t } = useLocaleText();

  React.useEffect(() => {
    async function load() {
      const response = await fetch("/v1/regions", { cache: "no-store" });
      if (!response.ok) return;
      const nextRegions = (await response.json()) as RegionOption[];
      setRegions(nextRegions);
      setSelected(currentRegionName(nextRegions) || nextRegions[0]?.name || "");
    }
    load().catch(console.error);
  }, []);

  if (regions.length === 0) return null;

  return (
    <Select
      selectedKey={selected || null}
      aria-label={t("nav.router")}
      fullWidth={false}
      className="shrink-0"
      onSelectionChange={(key: React.Key | null) => {
        const name = String(key || "");
        if (!name) return;
        setSelected(name);
        const region = regions.find((item) => item.name === name);
        const href = region ? normalizeRegionUrl(region.url) : "";
        if (href) window.location.href = href;
      }}
    >
      <Select.Trigger
        className={cn(
          "h-7 min-h-7 w-auto max-w-[9rem] items-center gap-1 rounded-md border-0 bg-transparent px-1.5 py-0",
          "text-[11px] font-medium tracking-wide text-muted-foreground shadow-none",
          "hover:bg-transparent hover:text-foreground focus:bg-transparent focus-visible:ring-0",
          "data-[pressed=true]:bg-transparent data-[focus-visible=true]:ring-0",
        )}
      >
        <Select.Value className="block min-w-0 truncate text-[11px] font-medium tracking-wide text-inherit">
          {selected || t("nav.router")}
        </Select.Value>
        <Select.Indicator className="size-3.5 shrink-0 text-muted-foreground/60" />
      </Select.Trigger>
      <Select.Popover className="min-w-[8.5rem] rounded-lg border border-border/70 bg-background p-1 shadow-md">
        <ListBox aria-label={t("nav.routers")} className="outline-none">
          {regions.map((region) => (
            <ListBox.Item
              key={region.name}
              id={region.name}
              textValue={region.name}
              className="rounded-md px-2.5 py-1.5 text-xs text-foreground outline-none data-[focused=true]:bg-muted data-[selected=true]:font-semibold"
            >
              {region.name}
            </ListBox.Item>
          ))}
        </ListBox>
      </Select.Popover>
    </Select>
  );
}

function LanguageSwitcher() {
  const { locale, setLocale, t } = useLocaleText();
  const showEnglish = locale !== "en";

  return (
    <button
      type="button"
      aria-label={t("common.language")}
      className="inline-flex h-8 shrink-0 items-center justify-center rounded-md px-1.5 text-xs font-medium tracking-wide text-muted-foreground transition-colors hover:text-foreground"
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
      ? [{ id: "account" as const, href: DASHBOARD_ACCOUNT_API_KEYS_PATH, icon: UserRound, label: t("nav.accountTab") }]
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
              "inline-flex h-8 min-w-0 items-center gap-1.5 rounded-md px-2 text-xs transition-colors",
              selected
                ? "font-semibold text-foreground"
                : "font-medium text-muted-foreground hover:text-foreground",
            )}
          >
            <Icon
              className={cn(
                "h-3.5 w-3.5 shrink-0",
                selected ? "text-muted-foreground" : "text-muted-foreground/70",
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
  const authed = !!session?.authenticated;
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
        Three-column topbar: brand (logo + region) | centered section tabs | lang + user.
        Language control only offers the alternate locale (EN while zh-CN, 中 while en).
      */}
      <header className="mx-auto w-[calc(100%-2rem)] max-w-7xl py-4">
        <div className="grid grid-cols-[minmax(0,1fr)_auto_minmax(0,1fr)] items-center gap-x-3 gap-y-2">
          <div className="flex min-w-0 items-center gap-1.5 justify-self-start">
            <Link href={DASHBOARD_CLIENTS_PATH} className="flex shrink-0 items-center" aria-label="CC-Switch Router">
              <Image src="/router-logo.svg" alt="" width={32} height={32} className="h-8 w-8 shrink-0" priority />
            </Link>
            <span className="hidden h-3 w-px shrink-0 bg-border/80 sm:block" aria-hidden />
            <RouterSwitcher />
          </div>

          {showDashboardNav ? (
            <div className="min-w-0 justify-self-center overflow-x-auto [-ms-overflow-style:none] [scrollbar-width:none] [&::-webkit-scrollbar]:hidden">
              <DashboardNav active={active} authed={authed} />
            </div>
          ) : (
            <div />
          )}

          <div className="flex flex-nowrap items-center justify-end gap-2 justify-self-end">
            <LanguageSwitcher />
            {authed ? (
              <Dropdown>
                <Dropdown.Trigger className="shrink-0 outline-none">
                  <Button
                    variant="ghost"
                    size="sm"
                    className="h-8 max-w-[12rem] gap-1.5 px-2.5 text-xs font-medium text-muted-foreground hover:text-foreground whitespace-nowrap [&_svg]:my-0"
                  >
                    <UserRound className="h-3.5 w-3.5 shrink-0 text-muted-foreground/70" />
                    <span className="hidden min-w-0 truncate sm:inline">{session?.user?.email}</span>
                  </Button>
                </Dropdown.Trigger>
                <Dropdown.Popover placement="bottom right">
                  <Dropdown.Menu aria-label={t("nav.userMenu")}>
                    <Dropdown.Section>
                      <Dropdown.Item id="email" isDisabled className="text-xs text-muted-foreground">
                        {session?.user?.email}
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
                className="h-8 shrink-0 px-2.5 text-xs font-medium text-muted-foreground hover:text-foreground"
                onClick={() => setLoginOpen(true)}
                isDisabled={loading}
              >
                {t("nav.login")}
              </Button>
            )}
          </div>
        </div>
      </header>

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
    <LocaleProvider>
      <AuthProvider>
        <DashboardDataProvider enabled={dashboardDataEnabled}>
          <div className="flex min-h-dvh min-w-0 flex-col">
            <Topbar active={active} />
            <div className="flex min-w-0 flex-1 flex-col">{children}</div>
          </div>
          <AnnouncementDialog />
          <Toast.Provider placement="top end" />
        </DashboardDataProvider>
      </AuthProvider>
    </LocaleProvider>
  );
}
