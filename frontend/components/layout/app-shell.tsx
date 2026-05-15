"use client";

import Image from "next/image";
import Link from "next/link";
import { Button, Dropdown, ListBox, Select } from "@heroui/react";
import { LogOut, Settings, UserRound } from "lucide-react";
import * as React from "react";
import { LoginDialog } from "@/components/auth/login-dialog";
import { AuthProvider, useAuth } from "@/components/auth/auth-provider";
import { getDashboard } from "@/lib/api";
import type { DashboardResponse } from "@/lib/types";
import { formatNumber } from "@/lib/utils";

type RegionOption = {
  name: string;
  url: string;
};

function countDistinctCountries(data: DashboardResponse | null) {
  const set = new Set<string>();
  if (data?.map?.server?.countryCode) set.add(data.map.server.countryCode);
  for (const client of data?.map?.clients || []) {
    if (client.countryCode) set.add(client.countryCode);
  }
  return set.size;
}

function TopbarStats() {
  const [data, setData] = React.useState<DashboardResponse | null>(null);

  const load = React.useCallback(async () => {
    setData(await getDashboard());
  }, []);

  React.useEffect(() => {
    load().catch(console.error);
    const id = window.setInterval(() => load().catch(console.error), 5000);
    return () => window.clearInterval(id);
  }, [load]);

  return (
    <div className="hidden flex-wrap items-center justify-end gap-2 text-xs text-muted-foreground lg:flex">
      <span title="Total number of clients registered on this router.">
        <strong className="text-foreground">{formatNumber(data?.stats?.clients || 0)}</strong> clients
      </span>
      <span className="opacity-40">·</span>
      <span title="Distinct countries currently routing traffic through this router.">
        <strong className="text-foreground">{formatNumber(countDistinctCountries(data))}</strong> countries
      </span>
      <span className="opacity-40">·</span>
      <span title="Clients whose share status is currently active.">
        <strong className="text-foreground">{formatNumber(data?.stats?.activeShares || 0)}</strong> active shares
      </span>
      <span className="opacity-40">·</span>
      <span title="Total HTTP requests currently in-flight across every share.">
        <strong className="text-foreground">{formatNumber(data?.stats?.totalActiveRequests || 0)}</strong> in-flight requests
      </span>
    </div>
  );
}

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

function RouterSwitcher() {
  const [regions, setRegions] = React.useState<RegionOption[]>([]);
  const [selected, setSelected] = React.useState("");

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
      aria-label="Router"
      className="hidden sm:flex"
      onSelectionChange={(key) => {
        const name = String(key || "");
        if (!name) return;
          setSelected(name);
          const region = regions.find((item) => item.name === name);
          const href = region ? normalizeRegionUrl(region.url) : "";
          if (href) window.location.href = href;
      }}
    >
      <Select.Trigger className="min-h-8 items-center gap-2 rounded-lg border border-border bg-card px-2.5 py-1.5 text-xs text-foreground shadow-none">
        <span className="font-semibold text-foreground">Router</span>
        <Select.Value className="max-w-32 truncate text-xs font-normal text-foreground">
          {selected || "Router"}
        </Select.Value>
        <Select.Indicator className="text-muted-foreground" />
      </Select.Trigger>
      <Select.Popover className="min-w-40">
        <ListBox aria-label="Routers">
          {regions.map((region) => (
            <ListBox.Item key={region.name} id={region.name} textValue={region.name}>
              {region.name}
            </ListBox.Item>
          ))}
        </ListBox>
      </Select.Popover>
    </Select>
  );
}

function Topbar({ active }: { active: "dashboard" | "settings" }) {
  const { session, loading, logout } = useAuth();
  const [loginOpen, setLoginOpen] = React.useState(false);
  const authed = !!session?.authenticated;

  return (
    <header className="mx-auto flex w-[calc(100%-2rem)] max-w-7xl items-center justify-between gap-4 py-5">
      <Link href="/" className="flex items-center gap-3">
        <Image src="/router-logo.svg" alt="" width={36} height={36} className="h-9 w-9" priority />
        <span className="text-base font-extrabold leading-none">CC-Switch Router</span>
      </Link>
      <RouterSwitcher />
      <div className="flex flex-1 items-center justify-end gap-4">
        {active === "dashboard" ? <TopbarStats /> : null}
        {authed ? (
          <Dropdown>
            <Dropdown.Trigger>
              <Button variant="outline" size="sm" className="gap-2">
                <UserRound className="h-4 w-4" />
                <span className="hidden max-w-48 truncate sm:inline">{session?.user?.email}</span>
              </Button>
            </Dropdown.Trigger>
            <Dropdown.Popover placement="bottom right">
              <Dropdown.Menu aria-label="User menu">
                <Dropdown.Section>
                  <Dropdown.Item id="email" isDisabled className="text-xs text-muted-foreground">
                    {session?.user?.email}
                  </Dropdown.Item>
                </Dropdown.Section>
                {session?.isAdmin ? (
                  <Dropdown.Item id="settings" href="/settings/" target="_blank" rel="noopener noreferrer">
                    <Settings className="h-4 w-4" />
                    Settings
                  </Dropdown.Item>
                ) : null}
                <Dropdown.Item id="logout" onAction={() => logout().catch(console.error)} className="text-destructive">
                  <LogOut className="h-4 w-4" />
                  Logout
                </Dropdown.Item>
              </Dropdown.Menu>
            </Dropdown.Popover>
          </Dropdown>
        ) : (
          <Button
            variant="outline"
            size="sm"
            className="h-8 px-3 text-[11px] font-normal text-muted-foreground hover:text-slate-500"
            onClick={() => setLoginOpen(true)}
            isDisabled={loading}
          >
            Login
          </Button>
        )}
      </div>
      <LoginDialog open={loginOpen} onOpenChange={setLoginOpen} />
    </header>
  );
}

export function AppShell({
  active,
  children,
}: {
  active: "dashboard" | "settings";
  children: React.ReactNode;
}) {
  return (
    <AuthProvider>
      <Topbar active={active} />
      {children}
    </AuthProvider>
  );
}
