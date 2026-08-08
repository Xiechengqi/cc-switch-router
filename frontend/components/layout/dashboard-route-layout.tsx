"use client";

import { usePathname } from "next/navigation";
import * as React from "react";
import { AppShell } from "@/components/layout/app-shell";
import { DashboardLayout } from "@/components/layout/dashboard-layout";
import { pathnameForDashboardShell } from "@/lib/dashboard-nav";

export function DashboardRouteLayout({ children }: { children: React.ReactNode }) {
  const pathname = usePathname() || "/clients/";
  const active = pathnameForDashboardShell(pathname);
  const previousPathnameRef = React.useRef(pathname);

  React.useEffect(() => {
    if (previousPathnameRef.current === pathname) return;
    previousPathnameRef.current = pathname;
    const frame = window.requestAnimationFrame(() => {
      document.getElementById("main-content")?.focus({ preventScroll: true });
    });
    return () => window.cancelAnimationFrame(frame);
  }, [pathname]);

  return (
    <AppShell active={active}>
      <DashboardLayout>{children}</DashboardLayout>
    </AppShell>
  );
}
