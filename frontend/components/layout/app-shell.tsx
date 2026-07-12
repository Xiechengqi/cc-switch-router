"use client";

import Image from "next/image";
import Link from "next/link";
import { Button, Dropdown, ListBox, Modal, Select, Tabs } from "@heroui/react";
import { Activity, Copy, Eye, EyeOff, KeyRound, Loader2, LogOut, RotateCcw, Settings, UserRound } from "lucide-react";
import * as React from "react";
import { LoginDialog } from "@/components/auth/login-dialog";
import { Toast } from "@heroui/react";
import { AuthProvider, useAuth } from "@/components/auth/auth-provider";
import { LocaleProvider, useLocaleText } from "@/components/i18n/locale-provider";
import { refreshAccessToken } from "@/lib/auth";
import { getUserApiToken, resetUserApiToken } from "@/lib/api";
import { DashboardDataProvider, useDashboardData } from "@/components/dashboard/dashboard-data";
import type { AppLocale } from "@/lib/i18n";
import type { UserApiTokenStatus } from "@/lib/types";

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

function RouterSwitcher({ onNameChange }: { onNameChange?: (name: string) => void }) {
  const [regions, setRegions] = React.useState<RegionOption[]>([]);
  const [selected, setSelected] = React.useState("");
  const { t } = useLocaleText();

  React.useEffect(() => {
    async function load() {
      const response = await fetch("/v1/regions", { cache: "no-store" });
      if (!response.ok) return;
      const nextRegions = (await response.json()) as RegionOption[];
      setRegions(nextRegions);
      const next = currentRegionName(nextRegions) || nextRegions[0]?.name || "";
      setSelected(next);
      onNameChange?.(next);
    }
    load().catch(console.error);
  }, []);

  if (regions.length === 0) return null;

  return (
    <Select
      selectedKey={selected || null}
      aria-label={t("nav.router")}
      className="hidden sm:flex"
      onSelectionChange={(key: React.Key | null) => {
        const name = String(key || "");
        if (!name) return;
        setSelected(name);
        onNameChange?.(name);
        const region = regions.find((item) => item.name === name);
        const href = region ? normalizeRegionUrl(region.url) : "";
        if (href) window.location.href = href;
      }}
    >
      <Select.Trigger className="min-h-8 w-36 items-center rounded-lg border border-border bg-card py-1.5 pl-2.5 pr-8 text-xs text-foreground shadow-none">
        <Select.Value className="block min-w-0 max-w-[7.5rem] truncate pr-1 text-xs font-normal text-foreground">
          {selected || t("nav.router")}
        </Select.Value>
        <Select.Indicator className="text-muted-foreground" />
      </Select.Trigger>
      <Select.Popover className="min-w-40">
        <ListBox aria-label={t("nav.routers")}>
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

function LanguageSwitcher() {
  const { locale, setLocale, t } = useLocaleText();
  return (
    <Tabs
      selectedKey={locale}
      aria-label={t("common.language")}
      variant="secondary"
      className="text-foreground"
      onSelectionChange={(key: React.Key) => {
        if (key === "en" || key === "zh-CN") setLocale(key as AppLocale);
      }}
    >
      <Tabs.List className="grid grid-cols-2 text-foreground">
        <Tabs.Tab id="en" className="px-2 text-xs text-muted-foreground data-[selected=true]:text-foreground">{t("common.english")}</Tabs.Tab>
        <Tabs.Tab id="zh-CN" className="px-2 text-xs text-muted-foreground data-[selected=true]:text-foreground">{t("common.chinese")}</Tabs.Tab>
      </Tabs.List>
    </Tabs>
  );
}

function ApiTokenDialog({ open, onOpenChange }: { open: boolean; onOpenChange: (open: boolean) => void }) {
  const [token, setToken] = React.useState<UserApiTokenStatus | null>(null);
  const [rawToken, setRawToken] = React.useState("");
  const [showToken, setShowToken] = React.useState(false);
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState("");
  const [copied, setCopied] = React.useState(false);
  const maskedToken = React.useMemo(() => {
    if (rawToken) {
      if (rawToken.length <= 12) return "•".repeat(rawToken.length);
      return `${rawToken.slice(0, 8)}${"•".repeat(16)}${rawToken.slice(-4)}`;
    }
    if (token?.prefix) return `${token.prefix}${"•".repeat(16)}`;
    return "Reset to generate a new API token";
  }, [rawToken, token?.prefix]);

  const load = React.useCallback(async () => {
    setBusy(true);
    setError("");
    try {
      const response = await getUserApiToken();
      setToken(response.token);
      setRawToken(response.apiToken || "");
      setShowToken(false);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }, []);

  React.useEffect(() => {
    if (!open) return;
    setRawToken("");
    setShowToken(false);
    setCopied(false);
    load().catch(console.error);
  }, [load, open]);

  const reset = async () => {
    setBusy(true);
    setError("");
    setCopied(false);
    try {
      const response = await resetUserApiToken();
      setToken(response.token);
      setRawToken(response.apiToken);
      setShowToken(false);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const copy = async () => {
    if (!rawToken) return;
    await navigator.clipboard.writeText(rawToken);
    setCopied(true);
    window.setTimeout(() => setCopied(false), 1500);
  };

  return (
    <Modal isOpen={open} onOpenChange={onOpenChange}>
      <Modal.Backdrop>
        <Modal.Container placement="center">
          <Modal.Dialog className="light w-[min(560px,calc(100vw-2rem))] max-w-none !bg-white !text-slate-900 [--foreground:rgb(15,23,42)] [--muted:rgb(100,116,139)] [--overlay:#fff] [--overlay-foreground:rgb(15,23,42)] [--surface:#fff] [--surface-foreground:rgb(15,23,42)]">
            <Modal.CloseTrigger className="!bg-slate-100 !text-slate-700 hover:!bg-slate-200 hover:!text-slate-950" />
            <Modal.Header>
              <div>
                <Modal.Heading>API Token</Modal.Heading>
                <p className="mt-1 text-sm text-slate-600">
                  用它调用 router API，也可作为 share 调用的 `Authorization: Bearer ...`。
                </p>
              </div>
            </Modal.Header>
            <Modal.Body className="grid gap-4">
              {error ? <div className="rounded-md border border-red-200 bg-red-50 px-3 py-2 text-sm text-red-700">{error}</div> : null}
              <div className="grid gap-2 rounded-lg border border-slate-200 bg-slate-50 p-3 text-sm text-slate-900">
                <div className="flex justify-between gap-3">
                  <span className="text-slate-500">Prefix</span>
                  <strong className="font-mono">{token?.prefix || (busy ? "loading..." : "-")}</strong>
                </div>
                <div className="flex justify-between gap-3">
                  <span className="text-slate-500">Created</span>
                  <span>{token?.createdAt ? new Date(token.createdAt).toLocaleString() : "-"}</span>
                </div>
                <div className="flex justify-between gap-3">
                  <span className="text-slate-500">Last used</span>
                  <span>{token?.lastUsedAt ? new Date(token.lastUsedAt).toLocaleString() : "-"}</span>
                </div>
                <div className="flex justify-between gap-3">
                  <span className="text-slate-500">Scopes</span>
                  <span className="text-right">{token?.scopes?.join(", ") || "-"}</span>
                </div>
              </div>
              <div className="grid gap-2">
                <span className="text-xs text-slate-500">
                  默认脱敏显示。点击小眼睛可查看完整 API token；旧 token 如未保存明文，请重置后查看。
                </span>
                <div className="flex items-start gap-2 rounded-lg border border-slate-200 bg-white px-3 py-2 text-slate-900">
                  <div className="min-w-0 flex-1 break-all font-mono text-xs">
                    {showToken && rawToken ? rawToken : maskedToken}
                  </div>
                  <button
                    type="button"
                    aria-label={showToken ? "Hide API token" : "Show API token"}
                    title={showToken ? "Hide API token" : "Show API token"}
                    className="inline-flex h-7 w-7 shrink-0 items-center justify-center rounded-md text-slate-500 transition-colors hover:bg-slate-100 hover:text-slate-900 disabled:cursor-not-allowed disabled:opacity-40"
                    disabled={!rawToken}
                    onClick={() => setShowToken((value) => !value)}
                  >
                    {showToken ? <EyeOff className="h-4 w-4" /> : <Eye className="h-4 w-4" />}
                  </button>
                </div>
              </div>
            </Modal.Body>
            <Modal.Footer>
              <Button variant="outline" onClick={copy} isDisabled={!rawToken || busy}>
                <Copy className="h-4 w-4" />
                {copied ? "已复制" : "复制"}
              </Button>
              <Button variant="primary" onClick={reset} isDisabled={busy}>
                {busy ? <Loader2 className="h-4 w-4 animate-spin" /> : <RotateCcw className="h-4 w-4" />}
                重置并显示
              </Button>
            </Modal.Footer>
          </Modal.Dialog>
        </Modal.Container>
      </Modal.Backdrop>
    </Modal>
  );
}

function Topbar({ active }: { active: "dashboard" | "settings" | "metrics" }) {
  const { session, loading, logout } = useAuth();
  const { t } = useLocaleText();
  const [loginOpen, setLoginOpen] = React.useState(false);
  const [apiTokenOpen, setApiTokenOpen] = React.useState(false);
  const [clientRedirect, setClientRedirect] = React.useState<string | null>(null);
  const [routerName, setRouterName] = React.useState("");
  const dashboard = useDashboardData();
  const redirectStartedRef = React.useRef(false);
  const authed = !!session?.authenticated;

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
    <header className="mx-auto flex w-[calc(100%-2rem)] max-w-7xl items-center justify-between gap-4 py-5">
      <Link href="/" className="flex items-center gap-3">
        <Image src="/router-logo.svg" alt="" width={36} height={36} className="h-9 w-9" priority />
        <span className="grid gap-1">
          <span className="text-base font-extrabold leading-none">CC-Switch Router</span>
          <span className="font-mono text-[10px] uppercase tracking-[0.14em] text-muted-foreground">
            {routerName || t("nav.router")}{active === "dashboard" ? <> · <span className={dashboard.fresh ? "text-emerald-700" : "text-amber-700"}>{dashboard.fresh ? "LIVE" : "STALE"}</span></> : null}
          </span>
        </span>
      </Link>
      <div className="flex flex-1 items-center justify-end gap-4">
        <RouterSwitcher onNameChange={setRouterName} />
        <LanguageSwitcher />
        {authed ? (
          <Dropdown>
            <Dropdown.Trigger>
              <Button variant="outline" size="sm" className="gap-2">
                <UserRound className="h-4 w-4" />
                <span className="hidden max-w-48 truncate sm:inline">{session?.user?.email}</span>
              </Button>
            </Dropdown.Trigger>
            <Dropdown.Popover placement="bottom right">
              <Dropdown.Menu aria-label={t("nav.userMenu")}>
                <Dropdown.Section>
                  <Dropdown.Item id="email" isDisabled className="text-xs text-muted-foreground">
                    {session?.user?.email}
                  </Dropdown.Item>
                </Dropdown.Section>
                <Dropdown.Item id="api-token" onAction={() => setApiTokenOpen(true)}>
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
            variant="outline"
            size="sm"
            className="h-8 px-3 text-[11px] font-normal text-muted-foreground hover:text-slate-500"
            onClick={() => setLoginOpen(true)}
            isDisabled={loading}
          >
            {t("nav.login")}
          </Button>
        )}
      </div>
      <LoginDialog open={loginOpen} onOpenChange={setLoginOpen} />
      <ApiTokenDialog open={apiTokenOpen} onOpenChange={setApiTokenOpen} />
    </header>
  );
}

export function AppShell({
  active,
  children,
}: {
  active: "dashboard" | "settings" | "metrics";
  children: React.ReactNode;
}) {
  return (
    <LocaleProvider>
      <AuthProvider>
        <DashboardDataProvider enabled={active === "dashboard"}>
          <Topbar active={active} />
          {children}
          <Toast.Provider placement="top end" />
        </DashboardDataProvider>
      </AuthProvider>
    </LocaleProvider>
  );
}
