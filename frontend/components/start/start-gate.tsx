"use client";

import Image from "next/image";
import { usePathname, useRouter } from "next/navigation";
import { Loader2 } from "lucide-react";
import * as React from "react";
import { useAuth } from "@/components/auth/auth-provider";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { StartPage } from "@/components/start/start-page";
import { homeEntryStatus } from "@/lib/auth";
import { DASHBOARD_CLIENTS_PATH, buildDashboardHref, defaultDashboardRouteFromSearch } from "@/lib/dashboard-nav";
import { isGatedHomePath, isHomeTaskAddress, pathForHomeEntry, type HomeEntryStatus } from "@/lib/home-entry";

function LoadingScreen() {
  const { t } = useLocaleText();
  return (
    <main className="grid min-h-dvh place-items-center px-4" aria-busy="true">
      <div className="grid justify-items-center gap-3 text-center" role="status" aria-live="polite">
        <Image src="/router-logo.svg" alt="" width={40} height={40} priority />
        <div className="flex items-center gap-2 text-sm font-medium text-muted-foreground">
          <Loader2 className="h-4 w-4 animate-spin" aria-hidden />
          {t("common.loading")}
        </div>
      </div>
    </main>
  );
}

export function StartGate({ children }: { children: React.ReactNode }) {
  const pathname = usePathname() || "/";
  const router = useRouter();
  const { session, loading } = useAuth();
  const [home, setHome] = React.useState<HomeEntryStatus | "failed" | null>(null);
  const [search, setSearch] = React.useState("");
  const [searchReady, setSearchReady] = React.useState(false);
  React.useEffect(() => {
    setSearch(window.location.search);
    setSearchReady(true);
  }, [pathname]);
  const candidate = isGatedHomePath(pathname);
  const taskOnRoot = searchReady && pathname === "/" && isHomeTaskAddress(pathname, search);
  const gated = searchReady && candidate && !isHomeTaskAddress(pathname, search);
  const authed = !!session?.authenticated;
  const actor = session?.user?.id || "";

  React.useEffect(() => {
    if (!gated || loading || authed || pathname === DASHBOARD_CLIENTS_PATH) return;
    router.replace(DASHBOARD_CLIENTS_PATH);
  }, [authed, gated, loading, pathname, router]);

  React.useEffect(() => {
    if (!gated || loading || !authed) return;
    let active = true;
    setHome(null);
    homeEntryStatus()
      .then((status) => { if (active) setHome(status); })
      .catch(() => { if (active) setHome("failed"); });
    return () => { active = false; };
  }, [actor, authed, gated, loading]);

  React.useEffect(() => {
    if (!taskOnRoot) return;
    router.replace(buildDashboardHref(defaultDashboardRouteFromSearch(search), search));
  }, [router, search, taskOnRoot]);

  React.useEffect(() => {
    if (!gated || pathname === DASHBOARD_CLIENTS_PATH || home === null) return;
    const entry = home === "failed" ? null : home.entry;
    const seen = home !== "failed" && (home.hasActivity || entry !== null);
    if (!seen && home !== "failed") return;
    router.replace(entry ? pathForHomeEntry(entry) : DASHBOARD_CLIENTS_PATH);
  }, [gated, home, pathname, router]);

  if (!candidate) return children;
  if (!searchReady) return <LoadingScreen />;
  if (taskOnRoot) return children;
  if (!gated) return children;
  if (loading || (authed && home === null)) return <LoadingScreen />;
  if (!authed) return pathname === DASHBOARD_CLIENTS_PATH ? children : <LoadingScreen />;
  if (!home || home === "failed") return pathname === DASHBOARD_CLIENTS_PATH ? children : <LoadingScreen />;
  if (home.hasActivity || home.entry) return pathname === DASHBOARD_CLIENTS_PATH ? children : <LoadingScreen />;
  return <StartPage />;
}
